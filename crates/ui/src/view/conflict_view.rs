//! Disegno dell'editor dei conflitti (S30): le tre versioni in alto, il risultato modificabile
//! in basso con i pulsanti per blocco. Le funzioni leggono il modello
//! ([`crate::conflict`]) e producono [`Action`]; non cambiano niente direttamente.

use super::stage_view::conflict_name;
use crate::Action;
use crate::conflict::{BlockInfo, Column, Editor, Load, Loaded, Pending, Pick};
use egui::{RichText, Ui};
use project_domain::Choice;

/// Altezza di una riga di pulsanti di un blocco.
const BLOCK_ROW: f32 = 24.0;
/// Quante righe di blocchi si vedono al massimo; le altre scorrendo.
const BLOCK_ROWS: f32 = 4.0;

/// L'editor del file in conflitto, al posto del grafo. `incoming`: il nome di ciò che sta
/// entrando (per l'intestazione di "loro" se il marcatore non lo dice). `can_write`: nessuna
/// scrittura né dialogo in corso.
pub(crate) fn conflict_editor(
    ui: &mut Ui,
    editor: &Editor,
    incoming: Option<&str>,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.heading("Conflitto");
        ui.label(RichText::new(editor.path.to_string()).monospace().strong());
        if let Some(loaded) = editor.loaded() {
            ui.weak(conflict_name(loaded.shown.kind));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let close = ui
                .add_enabled(!editor.sending, egui::Button::new("Chiudi"))
                .on_hover_text("Torna alla lista: le scelte non segnate come risolte si perdono");
            if close.clicked() {
                actions.push(Action::CloseConflict);
            }
            let restart = ui
                .add_enabled(!editor.sending, egui::Button::new("Ricomincia"))
                .on_hover_text("Rilegge il file dal disco: le scelte fatte qui si perdono");
            if restart.clicked() {
                actions.push(Action::RestartConflict);
            }
        });
    });
    ui.separator();
    match &editor.load {
        Load::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Lettura delle versioni…");
            });
        }
        Load::Failed(error) => {
            ui.add(
                egui::Label::new(
                    RichText::new(format!("Versioni non lette: {error}"))
                        .color(ui.visuals().error_fg_color),
                )
                .wrap(),
            );
            if ui.button("Riprova").clicked() {
                actions.push(Action::RestartConflict);
            }
        }
        Load::Ready(loaded) => loaded_editor(ui, editor, loaded, incoming, can_write, actions),
    }
}

fn loaded_editor(
    ui: &mut Ui,
    editor: &Editor,
    loaded: &Loaded,
    incoming: Option<&str>,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let footer = 64.0;
    let top = ((ui.available_height() - footer) * 0.38).max(80.0);
    let ours = loaded.ours_label.as_deref().unwrap_or("HEAD");
    let theirs = loaded
        .theirs_label
        .as_deref()
        .or(incoming)
        .unwrap_or("in ingresso");
    ui.columns(3, |cols| {
        column(
            &mut cols[0],
            "base",
            "Base (antenato comune)",
            &loaded.base,
            top,
        );
        column(
            &mut cols[1],
            "ours",
            &format!("Nostra ({ours})"),
            &loaded.ours,
            top,
        );
        column(
            &mut cols[2],
            "theirs",
            &format!("Loro ({theirs})"),
            &loaded.theirs,
            top,
        );
    });
    ui.separator();
    let enabled = !editor.sending;
    match loaded.result() {
        Some(result) => {
            let blocks = loaded.blocks();
            ui.horizontal(|ui| {
                ui.strong("Risultato");
                ui.weak(match blocks.len() {
                    0 => "nessun blocco di conflitto".to_owned(),
                    1 => "1 blocco di conflitto".to_owned(),
                    n => format!("{n} blocchi di conflitto"),
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let on = enabled && !blocks.is_empty();
                    if ui
                        .add_enabled(on, egui::Button::new("Tutto loro"))
                        .on_hover_text("Ogni blocco con le righe loro")
                        .clicked()
                    {
                        actions.push(Action::ChooseAll(Choice::Theirs));
                    }
                    if ui
                        .add_enabled(on, egui::Button::new("Tutto nostra"))
                        .on_hover_text("Ogni blocco con le righe nostre")
                        .clicked()
                    {
                        actions.push(Action::ChooseAll(Choice::Ours));
                    }
                });
            });
            if let Some(error) = loaded.parse_error() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "I marcatori non si leggono più ({error}): niente scelte per blocco, \
                             il testo resta modificabile. «Ricomincia» rilegge il file"
                        ))
                        .color(ui.visuals().warn_fg_color),
                    )
                    .wrap(),
                );
            }
            block_rows(ui, &blocks, enabled, actions);
            let height = (ui.available_height() - footer).max(60.0);
            egui::ScrollArea::both()
                .id_salt("risultato-conflitto")
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut text = result.to_owned();
                    let edit = ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .id(ui.make_persistent_id(("risultato", editor.path.to_string())))
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(8)
                            .interactive(enabled),
                    );
                    if edit.changed() {
                        actions.push(Action::ConflictText(text));
                    }
                });
        }
        None => sides_only(ui, loaded, enabled && can_write, actions),
    }
    footer_row(ui, editor, loaded, can_write, actions);
}

/// Una delle tre versioni, sola lettura, righe virtualizzate.
fn column(ui: &mut Ui, id: &str, title: &str, column: &Column, height: f32) {
    ui.strong(title);
    let weak = |ui: &mut Ui, text: &str| {
        ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
            ui.weak(text);
        });
    };
    match column {
        Column::Missing => weak(ui, "(assente)"),
        Column::Binary(size) => weak(ui, &format!("(binario, {size} byte)")),
        Column::Text(lines) => {
            let row = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .corner_radius(3.0)
                .inner_margin(4)
                .show(ui, |ui| {
                    egui::ScrollArea::both()
                        .id_salt(("colonna-conflitto", id))
                        .max_height(height)
                        .auto_shrink([false, false])
                        .show_rows(ui, row, lines.len(), |ui, range| {
                            for line in &lines[range] {
                                ui.add(egui::Label::new(RichText::new(line).monospace()).extend());
                            }
                        });
                });
        }
    }
}

/// Una riga di pulsanti per ogni blocco del risultato attuale.
fn block_rows(ui: &mut Ui, blocks: &[BlockInfo], enabled: bool, actions: &mut Vec<Action>) {
    if blocks.is_empty() {
        return;
    }
    let height = BLOCK_ROW * (blocks.len() as f32).min(BLOCK_ROWS);
    egui::ScrollArea::vertical()
        .id_salt("blocchi-conflitto")
        .max_height(height)
        .auto_shrink([false, true])
        .show_rows(ui, BLOCK_ROW, blocks.len(), |ui, range| {
            for block in &blocks[range] {
                ui.horizontal(|ui| {
                    ui.set_height(BLOCK_ROW - 4.0);
                    ui.label(format!("Blocco {} (riga {}):", block.index + 1, block.line));
                    let mut choice = |ui: &mut Ui, text: &str, hint: &str, choice: Choice| {
                        if ui
                            .add_enabled(enabled, egui::Button::new(text).small())
                            .on_hover_text(hint)
                            .clicked()
                        {
                            actions.push(Action::ChooseBlock(block.index, choice));
                        }
                    };
                    choice(ui, "Nostra", "Tiene le righe nostre", Choice::Ours);
                    choice(ui, "Loro", "Tiene le righe loro", Choice::Theirs);
                    choice(
                        ui,
                        "Entrambe",
                        "Le righe nostre e poi le loro",
                        Choice::OursThenTheirs,
                    );
                    if block.has_base {
                        choice(ui, "Base", "Le righe dell'antenato comune", Choice::Base);
                    }
                });
            }
        });
}

/// Il file non si modifica qui (binario, assente, marcatori non leggibili): solo i lati interi.
fn sides_only(ui: &mut Ui, loaded: &Loaded, enabled: bool, actions: &mut Vec<Action>) {
    let why = match &loaded.shown.worktree {
        None => "Il file non c'è nel worktree.".to_owned(),
        Some(v) if v.text.is_none() => "Il file non è testo.".to_owned(),
        Some(_) => "I marcatori di conflitto del file non si leggono.".to_owned(),
    };
    ui.add(
        egui::Label::new(format!(
            "{why} Si può risolvere solo con una versione intera (il file attuale si salva prima \
             in un backup)."
        ))
        .wrap(),
    );
    ui.horizontal(|ui| {
        for (pick, text) in [(Pick::Ours, "Usa nostra"), (Pick::Theirs, "Usa loro")] {
            let available = loaded.side_text(pick).is_some();
            let button = ui
                .add_enabled(enabled && available, egui::Button::new(text))
                .on_hover_text("Scrive quella versione nel file e la mette in stage")
                .on_disabled_hover_text(if available {
                    "Aspettare l'operazione in corso"
                } else {
                    "Quella versione manca o non è testo"
                });
            if button.clicked() {
                actions.push(Action::UseSide(pick));
            }
        }
    });
}

/// In fondo: errore, conferma in due tempi, "Segna come risolto", "Elimina il file".
fn footer_row(
    ui: &mut Ui,
    editor: &Editor,
    loaded: &Loaded,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    if let Some(error) = &editor.error {
        ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap());
    }
    let enabled = can_write && !editor.sending;
    if let Some(pending) = editor.pending {
        let warn = ui.visuals().warn_fg_color;
        ui.horizontal_wrapped(|ui| {
            let (text, button) = match pending {
                Pending::Markers(n) => (
                    format!(
                        "Contiene ancora marcatori ({n} blocchi): risolvere comunque? Il file \
                         va in stage così com'è"
                    ),
                    "Risolvi comunque",
                ),
                Pending::Delete => (
                    "Il file si elimina dal worktree e l'eliminazione va in stage. Il file \
                     attuale si salva prima in un backup"
                        .to_owned(),
                    "Elimina il file",
                ),
            };
            ui.label(RichText::new(text).color(warn));
            let confirm =
                egui::Button::new(RichText::new(button).strong()).fill(warn.gamma_multiply(0.3));
            if ui.add_enabled(enabled, confirm).clicked() {
                actions.push(Action::ConfirmConflict);
            }
            if ui.button("Annulla").clicked() {
                actions.push(Action::CancelConflict);
            }
        });
        return;
    }
    ui.horizontal(|ui| {
        if loaded.result().is_some() {
            let button = ui
                .add_enabled(
                    enabled,
                    egui::Button::new(RichText::new("Segna come risolto").strong()),
                )
                .on_hover_text(
                    "Scrive il risultato nel file e lo mette in stage. Il file com'era si salva \
                     prima in un backup",
                )
                .on_disabled_hover_text("Aspettare l'operazione in corso (o chiudere il dialogo)");
            if button.clicked() {
                actions.push(Action::ResolveConflict);
            }
        }
        if loaded.can_delete() {
            let color = if enabled {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().weak_text_color()
            };
            let button = ui
                .add_enabled(
                    enabled,
                    egui::Button::new(RichText::new("Elimina il file…").color(color)),
                )
                .on_hover_text("Risolve il conflitto eliminando il file: chiede conferma");
            if button.clicked() {
                actions.push(Action::DeleteConflicted);
            }
        }
        if editor.sending {
            ui.spinner();
            ui.label("Risoluzione…");
        }
    });
}
