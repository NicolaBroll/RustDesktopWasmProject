//! L6 — interfaccia egui condivisa da desktop e web.
//!
//! Regola: i callback grafici non modificano il documento direttamente,
//! producono un'[`Action`] che passa da [`Registry::run`].

use egui::{Key, KeyboardShortcut, Modifiers};
use project_engine::{Document, Picked, Platform, Registry, Session};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const DOCUMENT_KEY: &str = "document";

const NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const REDO: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
const REDO_ALT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);

/// Ciò che l'utente ha chiesto durante il frame.
enum Action {
    Run(&'static str, Value),
    Open,
    Save,
    Quit,
}

pub struct App {
    session: Session,
    registry: Registry,
    platform: Arc<dyn Platform>,
    /// File scelto dall'utente; arriva in modo asincrono sul web.
    picked: Arc<Mutex<Option<Picked>>>,
    title_buffer: String,
    new_item: String,
    status: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, platform: Arc<dyn Platform>) -> Self {
        let document: Document = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, DOCUMENT_KEY))
            .unwrap_or_default();
        Self {
            title_buffer: document.title().to_owned(),
            session: Session::new(document),
            registry: Registry::builtin(),
            platform,
            picked: Arc::default(),
            new_item: String::new(),
            status: String::new(),
        }
    }

    fn run(&mut self, id: &str, params: Value) -> Option<Value> {
        match self.registry.run(id, &mut self.session, params) {
            Ok(out) => Some(out),
            Err(e) => {
                log::warn!("{id}: {e}");
                self.status = e.to_string();
                None
            }
        }
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Run(id, params) => {
                if self.run(id, params).is_some() {
                    self.status.clear();
                }
            }
            Action::Open => {
                let picked = Arc::clone(&self.picked);
                let ctx = ctx.clone();
                self.platform.pick_file(Box::new(move |file| {
                    if let Some(file) = file {
                        *picked.lock().expect("mutex avvelenato") = Some(file);
                        ctx.request_repaint();
                    }
                }));
            }
            Action::Save => {
                if let Some(out) = self.run("document.export", json!({})) {
                    let content = out["content"].as_str().unwrap_or_default();
                    let name = format!("{}.json", file_stem(self.session.document().title()));
                    self.platform.save_file(&name, content.as_bytes().to_vec());
                    self.status = format!("Salvato come {name}");
                }
            }
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn receive_picked_file(&mut self) {
        let Some(file) = self.picked.lock().expect("mutex avvelenato").take() else {
            return;
        };
        match String::from_utf8(file.bytes) {
            Ok(content) => {
                if self
                    .run("document.load", json!({ "content": content }))
                    .is_some()
                {
                    self.status = format!("Aperto {}", file.name);
                }
            }
            Err(_) => self.status = format!("{}: il file non è testo UTF-8", file.name),
        }
    }

    fn shortcuts(&self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        ctx.input_mut(|i| {
            if i.consume_shortcut(&NEW) {
                actions.push(Action::Run("document.new", json!({})));
            }
            if i.consume_shortcut(&OPEN) {
                actions.push(Action::Open);
            }
            if i.consume_shortcut(&SAVE) {
                actions.push(Action::Save);
            }
        });
        // Mentre si scrive in un campo, Ctrl+Z appartiene al campo di testo.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        ctx.input_mut(|i| {
            // Ctrl+Shift+Z va controllato prima di Ctrl+Z.
            if i.consume_shortcut(&REDO) || i.consume_shortcut(&REDO_ALT) {
                actions.push(Action::Run("edit.redo", json!({})));
            } else if i.consume_shortcut(&UNDO) {
                actions.push(Action::Run("edit.undo", json!({})));
            }
        });
    }

    fn menu_bar(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let caps = self.platform.capabilities();
        let shortcut = |s: &KeyboardShortcut| ui.ctx().format_shortcut(s);
        let (new, open, save, undo, redo) = (
            shortcut(&NEW),
            shortcut(&OPEN),
            shortcut(&SAVE),
            shortcut(&UNDO),
            shortcut(&REDO),
        );
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui
                    .add(egui::Button::new("Nuovo").shortcut_text(new))
                    .clicked()
                {
                    actions.push(Action::Run("document.new", json!({})));
                }
                if ui
                    .add(egui::Button::new("Apri…").shortcut_text(open))
                    .clicked()
                {
                    actions.push(Action::Open);
                }
                if ui
                    .add(egui::Button::new("Salva…").shortcut_text(save))
                    .clicked()
                {
                    actions.push(Action::Save);
                }
                if caps.window_control {
                    ui.separator();
                    if ui.button("Esci").clicked() {
                        actions.push(Action::Quit);
                    }
                }
            });
            ui.menu_button("Modifica", |ui| {
                let undo_btn = egui::Button::new("Annulla").shortcut_text(undo);
                if ui.add_enabled(self.session.can_undo(), undo_btn).clicked() {
                    actions.push(Action::Run("edit.undo", json!({})));
                }
                let redo_btn = egui::Button::new("Ripeti").shortcut_text(redo);
                if ui.add_enabled(self.session.can_redo(), redo_btn).clicked() {
                    actions.push(Action::Run("edit.redo", json!({})));
                }
            });
        });
    }

    fn document_view(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let doc = self.session.document();

        let title = ui.add(
            egui::TextEdit::singleline(&mut self.title_buffer)
                .font(egui::TextStyle::Heading)
                .desired_width(f32::INFINITY),
        );
        if title.lost_focus() && self.title_buffer != doc.title() {
            actions.push(Action::Run(
                "document.set_title",
                json!({ "title": self.title_buffer }),
            ));
        } else if !title.has_focus() {
            // Riallinea dopo undo, apertura, nuovo documento…
            doc.title().clone_into(&mut self.title_buffer);
        }
        ui.separator();

        ui.horizontal(|ui| {
            let input =
                ui.add(egui::TextEdit::singleline(&mut self.new_item).hint_text("Nuova voce"));
            let enter = input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if ui.button("Aggiungi").clicked() || enter {
                actions.push(Action::Run(
                    "item.add",
                    json!({ "text": std::mem::take(&mut self.new_item) }),
                ));
                input.request_focus();
            }
        });
        ui.add_space(4.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            if doc.items().is_empty() {
                ui.weak("Nessuna voce.");
            }
            for (index, item) in doc.items().iter().enumerate() {
                ui.horizontal(|ui| {
                    let mut done = item.done;
                    let text = egui::RichText::new(&item.text);
                    let text = if done {
                        text.strikethrough().weak()
                    } else {
                        text
                    };
                    if ui.checkbox(&mut done, text).changed() {
                        actions.push(Action::Run("item.toggle", json!({ "index": index })));
                    }
                    if ui.small_button("✖").on_hover_text("Elimina").clicked() {
                        actions.push(Action::Run("item.remove", json!({ "index": index })));
                    }
                });
            }
        });
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, DOCUMENT_KEY, self.session.document());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.receive_picked_file();

        let mut actions = Vec::new();
        self.shortcuts(ui.ctx(), &mut actions);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &mut actions));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::warn_if_debug_build(ui);
                });
            });
        });
        egui::CentralPanel::default().show(ui, |ui| self.document_view(ui, &mut actions));

        let ctx = ui.ctx().clone();
        for action in actions {
            self.apply(&ctx, action);
        }
    }
}

/// Nome file sicuro ricavato dal titolo.
fn file_stem(title: &str) -> String {
    let stem: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if stem.trim_matches('_').is_empty() {
        "documento".to_owned()
    } else {
        stem
    }
}

#[cfg(test)]
mod tests {
    use super::file_stem;

    #[test]
    fn file_stem_is_safe() {
        assert_eq!(file_stem("Lista spesa/2"), "Lista_spesa_2");
        assert_eq!(file_stem("../.."), "documento");
    }
}
