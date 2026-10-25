//! Drawing of the conflict editor (S30): the three versions on top, the editable result
//! below with per-block buttons. The functions read the model
//! ([`crate::conflict`]) and produce [`Action`]; they change nothing directly.

use super::stage_view::conflict_name;
use crate::Action;
use crate::conflict::{BlockInfo, Column, Editor, Load, Loaded, Pending, Pick};
use egui::{RichText, Ui};
use project_domain::Choice;
use project_i18n::tr;

/// Height of a row of buttons of a block.
const BLOCK_ROW: f32 = 24.0;
/// How many rows of blocks are visible at most; the others by scrolling.
const BLOCK_ROWS: f32 = 4.0;

/// The editor of the conflicted file, in place of the graph. `incoming`: the name of what is
/// coming in (for the "theirs" header if the marker does not say it). `can_write`: no
/// write or dialog in progress.
pub(crate) fn conflict_editor(
    ui: &mut Ui,
    editor: &Editor,
    incoming: Option<&str>,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.heading(tr!("conflict-title"));
        ui.label(RichText::new(editor.path.to_string()).monospace().strong());
        if let Some(loaded) = editor.loaded() {
            ui.weak(conflict_name(loaded.shown.kind));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let close = ui
                .add_enabled(!editor.sending, egui::Button::new(tr!("conflict-close")))
                .on_hover_text(tr!("conflict-close-hint"));
            if close.clicked() {
                actions.push(Action::CloseConflict);
            }
            let restart = ui
                .add_enabled(!editor.sending, egui::Button::new(tr!("conflict-restart")))
                .on_hover_text(tr!("conflict-restart-hint"));
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
                ui.label(tr!("conflict-loading"));
            });
        }
        Load::Failed(error) => {
            ui.add(
                egui::Label::new(
                    RichText::new(tr!("conflict-load-failed", error = error))
                        .color(ui.visuals().error_fg_color),
                )
                .wrap(),
            );
            if ui.button(tr!("conflict-retry")).clicked() {
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
    let theirs = match loaded.theirs_label.as_deref().or(incoming) {
        Some(label) => label.to_owned(),
        None => tr!("conflict-incoming"),
    };
    ui.columns(3, |cols| {
        column(
            &mut cols[0],
            "base",
            &tr!("conflict-column-base"),
            &loaded.base,
            top,
        );
        column(
            &mut cols[1],
            "ours",
            &tr!("conflict-column-ours", label = ours),
            &loaded.ours,
            top,
        );
        column(
            &mut cols[2],
            "theirs",
            &tr!("conflict-column-theirs", label = &theirs),
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
                ui.strong(tr!("conflict-result"));
                ui.weak(tr!("conflict-blocks", n = blocks.len()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let on = enabled && !blocks.is_empty();
                    if ui
                        .add_enabled(on, egui::Button::new(tr!("conflict-all-theirs")))
                        .on_hover_text(tr!("conflict-all-theirs-hint"))
                        .clicked()
                    {
                        actions.push(Action::ChooseAll(Choice::Theirs));
                    }
                    if ui
                        .add_enabled(on, egui::Button::new(tr!("conflict-all-ours")))
                        .on_hover_text(tr!("conflict-all-ours-hint"))
                        .clicked()
                    {
                        actions.push(Action::ChooseAll(Choice::Ours));
                    }
                });
            });
            if let Some(error) = loaded.parse_error() {
                ui.add(
                    egui::Label::new(
                        RichText::new(tr!("conflict-markers-unreadable", error = error))
                            .color(ui.visuals().warn_fg_color),
                    )
                    .wrap(),
                );
            }
            block_rows(ui, &blocks, enabled, actions);
            let height = (ui.available_height() - footer).max(60.0);
            egui::ScrollArea::both()
                .id_salt("conflict-result")
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut text = result.to_owned();
                    let edit = ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .id(ui.make_persistent_id(("result", editor.path.to_string())))
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

/// One of the three versions, read-only, virtualized rows.
fn column(ui: &mut Ui, id: &str, title: &str, column: &Column, height: f32) {
    ui.strong(title);
    let weak = |ui: &mut Ui, text: &str| {
        ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
            ui.weak(text);
        });
    };
    match column {
        Column::Missing => weak(ui, &tr!("conflict-missing")),
        Column::Binary(size) => weak(ui, &tr!("conflict-binary", size = *size)),
        Column::Text(lines) => {
            let row = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .corner_radius(3.0)
                .inner_margin(4)
                .show(ui, |ui| {
                    egui::ScrollArea::both()
                        .id_salt(("conflict-column", id))
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

/// A row of buttons for each block of the current result.
fn block_rows(ui: &mut Ui, blocks: &[BlockInfo], enabled: bool, actions: &mut Vec<Action>) {
    if blocks.is_empty() {
        return;
    }
    let height = BLOCK_ROW * (blocks.len() as f32).min(BLOCK_ROWS);
    egui::ScrollArea::vertical()
        .id_salt("conflict-blocks")
        .max_height(height)
        .auto_shrink([false, true])
        .show_rows(ui, BLOCK_ROW, blocks.len(), |ui, range| {
            for block in &blocks[range] {
                ui.horizontal(|ui| {
                    ui.set_height(BLOCK_ROW - 4.0);
                    ui.label(tr!(
                        "conflict-block",
                        n = block.index + 1,
                        line = block.line
                    ));
                    let mut choice = |ui: &mut Ui, text: &str, hint: &str, choice: Choice| {
                        if ui
                            .add_enabled(enabled, egui::Button::new(text).small())
                            .on_hover_text(hint)
                            .clicked()
                        {
                            actions.push(Action::ChooseBlock(block.index, choice));
                        }
                    };
                    choice(
                        ui,
                        &tr!("conflict-choice-ours"),
                        &tr!("conflict-choice-ours-hint"),
                        Choice::Ours,
                    );
                    choice(
                        ui,
                        &tr!("conflict-choice-theirs"),
                        &tr!("conflict-choice-theirs-hint"),
                        Choice::Theirs,
                    );
                    choice(
                        ui,
                        &tr!("conflict-choice-both"),
                        &tr!("conflict-choice-both-hint"),
                        Choice::OursThenTheirs,
                    );
                    if block.has_base {
                        choice(
                            ui,
                            &tr!("conflict-choice-base"),
                            &tr!("conflict-choice-base-hint"),
                            Choice::Base,
                        );
                    }
                });
            }
        });
}

/// The file is not edited here (binary, absent, unreadable markers): only whole sides.
fn sides_only(ui: &mut Ui, loaded: &Loaded, enabled: bool, actions: &mut Vec<Action>) {
    let why = match &loaded.shown.worktree {
        None => tr!("conflict-sides-missing"),
        Some(v) if v.text.is_none() => tr!("conflict-sides-not-text"),
        Some(_) => tr!("conflict-sides-unreadable"),
    };
    ui.add(egui::Label::new(why).wrap());
    ui.horizontal(|ui| {
        for (pick, text) in [
            (Pick::Ours, tr!("conflict-use-ours")),
            (Pick::Theirs, tr!("conflict-use-theirs")),
        ] {
            let available = loaded.side_text(pick).is_some();
            let button = ui
                .add_enabled(enabled && available, egui::Button::new(text))
                .on_hover_text(tr!("conflict-use-side-hint"))
                .on_disabled_hover_text(if available {
                    tr!("conflict-wait")
                } else {
                    tr!("conflict-side-unavailable")
                });
            if button.clicked() {
                actions.push(Action::UseSide(pick));
            }
        }
    });
}

/// At the bottom: error, two-step confirmation, "Mark as resolved", "Delete the file".
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
                    tr!("conflict-confirm-markers", n = n),
                    tr!("conflict-confirm-markers-button"),
                ),
                Pending::Delete => (
                    tr!("conflict-confirm-delete"),
                    tr!("conflict-confirm-delete-button"),
                ),
            };
            ui.label(RichText::new(text).color(warn));
            let confirm =
                egui::Button::new(RichText::new(button).strong()).fill(warn.gamma_multiply(0.3));
            if ui.add_enabled(enabled, confirm).clicked() {
                actions.push(Action::ConfirmConflict);
            }
            if ui.button(tr!("conflict-cancel")).clicked() {
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
                    egui::Button::new(RichText::new(tr!("conflict-mark-resolved")).strong()),
                )
                .on_hover_text(tr!("conflict-mark-resolved-hint"))
                .on_disabled_hover_text(tr!("conflict-wait-dialog"));
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
                    egui::Button::new(RichText::new(tr!("conflict-delete")).color(color)),
                )
                .on_hover_text(tr!("conflict-delete-hint"));
            if button.clicked() {
                actions.push(Action::DeleteConflicted);
            }
        }
        if editor.sending {
            ui.spinner();
            ui.label(tr!("conflict-resolving"));
        }
    });
}
