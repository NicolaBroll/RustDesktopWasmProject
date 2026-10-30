//! The interactive rebase editor (S33): the steps oldest first, dragged by the handle, with the
//! action selector, the message of the selected step and the preview of the result.

use super::*;
use crate::rebase::{self, Check, Editor, MessageSource, Phase};
use project_domain::RebaseAction;

/// Height of a step row and of a result row.
const STEP_HEIGHT: f32 = 26.0;
const RESULT_HEIGHT: f32 = 22.0;

/// The payload of a dragged step: its index in the plan.
struct StepDrag(usize);

pub(crate) fn rebase_editor(ctx: &egui::Context, editor: &Editor, actions: &mut Vec<Action>) {
    let width = (ctx.content_rect().width() - 48.0).clamp(320.0, 960.0);
    let modal = egui::Modal::new(egui::Id::unique("rebase-editor")).show(ctx, |ui| {
        ui.set_width(width);
        match &editor.phase {
            Phase::Loading => {
                ui.heading(tr!("rebase-heading-loading"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr!("rebase-loading"));
                });
                ui.add_space(8.0);
                if ui.button(tr!("rebase-close")).clicked() {
                    actions.push(Action::Rebase(rebase::Action::Close));
                }
            }
            Phase::Unavailable => {
                ui.heading(tr!("rebase-heading-loading"));
                ui.add_space(6.0);
                if let Some(error) = &editor.error {
                    ui.add(
                        egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color))
                            .wrap(),
                    );
                }
                ui.add_space(8.0);
                if ui.button(tr!("rebase-close")).clicked() {
                    actions.push(Action::Rebase(rebase::Action::Close));
                }
            }
            _ => plan_editor(ui, editor, width, actions),
        }
    });
    // Escape closes; a click outside does not (the edited plan would be lost by mistake).
    if editor.phase != Phase::Sending && !modal.backdrop_response.clicked() && modal.should_close()
    {
        if matches!(editor.phase, Phase::Confirm { .. }) {
            actions.push(Action::Rebase(rebase::Action::CancelConfirm));
        } else {
            actions.push(Action::Rebase(rebase::Action::Close));
        }
    }
    if let Phase::Confirm { reason } = &editor.phase {
        confirm_dialog(ctx, reason, actions);
    }
}

fn plan_editor(ui: &mut Ui, editor: &Editor, width: f32, actions: &mut Vec<Action>) {
    let editing = editor.phase == Phase::Editing;
    let check = editor.check();
    let bad_step = match &check {
        Check::Invalid { step, .. } => *step,
        _ => None,
    };
    ui.heading(tr!("rebase-heading", branch = &editor.branch));
    ui.add_space(4.0);
    ui.add(egui::Label::new(tr!("rebase-intro", what = &editor.what)).wrap());
    ui.add_space(6.0);
    let list_height = (ui.ctx().content_rect().height() * 0.45).clamp(120.0, 420.0);
    let steps_width = (width * 0.58).max(200.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(steps_width);
            ui.strong(tr!("rebase-steps"));
            ui.add_enabled_ui(editing, |ui| {
                steps(ui, editor, bad_step, list_height, actions);
            });
        });
        // No vertical separator: in a modal it would take all the height it is given.
        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.strong(tr!("rebase-result"));
            result(ui, editor, list_height);
        });
    });
    ui.separator();
    // A constant height below the lists: the modal does not move while the plan is edited.
    ui.vertical(|ui| {
        ui.set_min_height(130.0);
        ui.add_enabled_ui(editing, |ui| message_editor(ui, editor, actions));
        ui.add_space(6.0);
        if let Some(error) = &editor.error {
            ui.add(
                egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap(),
            );
        }
        match &check {
            Check::Invalid { text, .. } => {
                ui.add(
                    egui::Label::new(RichText::new(text).color(ui.visuals().warn_fg_color)).wrap(),
                );
            }
            Check::Noop => {
                ui.weak(tr!("rebase-noop"));
            }
            Check::Ready => {}
        }
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let run = ui
            .add_enabled(
                editing && check == Check::Ready,
                egui::Button::new(RichText::new(tr!("rebase-run")).strong()),
            )
            .on_hover_text(tr!("rebase-run-hint", branch = &editor.branch));
        if run.clicked() {
            actions.push(Action::Rebase(rebase::Action::Run));
        }
        if ui
            .add_enabled(
                editor.phase != Phase::Sending,
                egui::Button::new(tr!("rebase-close")),
            )
            .clicked()
        {
            actions.push(Action::Rebase(rebase::Action::Close));
        }
        if editor.phase == Phase::Sending {
            ui.spinner();
        }
    });
}

/// The steps, virtualized: handle, arrows, action, id, summary, published badge.
fn steps(
    ui: &mut Ui,
    editor: &Editor,
    bad_step: Option<usize>,
    height: f32,
    actions: &mut Vec<Action>,
) {
    let len = editor.rows.len();
    egui::ScrollArea::vertical()
        .id_salt("rebase-steps")
        .max_height(height)
        .auto_shrink([false, true])
        .show_rows(ui, STEP_HEIGHT, len, |ui, range| {
            for i in range {
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), STEP_HEIGHT), Sense::click());
                let visuals = ui.visuals();
                if bad_step == Some(i) {
                    ui.painter().rect_filled(
                        rect,
                        2.0,
                        visuals.error_fg_color.gamma_multiply(0.18),
                    );
                } else if editor.selected == Some(i) {
                    ui.painter()
                        .rect_filled(rect, 2.0, visuals.selection.bg_fill);
                } else if response.hovered() {
                    ui.painter()
                        .rect_filled(rect, 2.0, visuals.widgets.hovered.weak_bg_fill);
                }
                if response.clicked() {
                    actions.push(Action::Rebase(rebase::Action::Select(i)));
                }
                // Drop: into the gap above or below this row, by the pointer's half.
                if let Some(pointer) = ui.ctx().pointer_interact_pos()
                    && response.dnd_hover_payload::<StepDrag>().is_some()
                {
                    let below = pointer.y > rect.center().y;
                    let y = if below { rect.bottom() } else { rect.top() };
                    ui.painter().hline(
                        rect.x_range(),
                        y,
                        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                    );
                    if let Some(dragged) = response.dnd_release_payload::<StepDrag>() {
                        let to = if below { i + 1 } else { i };
                        actions.push(Action::Rebase(rebase::Action::Move {
                            from: dragged.0,
                            to,
                        }));
                    }
                }
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink2(vec2(4.0, 0.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                step_row(&mut child, editor, i, len, actions);
            }
        });
}

fn step_row(ui: &mut Ui, editor: &Editor, i: usize, len: usize, actions: &mut Vec<Action>) {
    let row = &editor.rows[i];
    let handle = ui.dnd_drag_source(
        egui::Id::unique(("rebase-step", row.commit)),
        StepDrag(i),
        |ui| {
            // Three lines drawn: the fonts have no handle glyph.
            let (rect, response) = ui.allocate_exact_size(vec2(14.0, STEP_HEIGHT), Sense::hover());
            let color = if response.hovered() {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            for dy in [-4.0, 0.0, 4.0] {
                let y = rect.center().y + dy;
                ui.painter()
                    .hline(rect.x_range().shrink(1.0), y, egui::Stroke::new(1.5, color));
            }
            response
        },
    );
    handle.response.on_hover_text(tr!("rebase-drag-hint"));
    if arrow(ui, true, i > 0)
        .on_hover_text(tr!("rebase-up-hint"))
        .clicked()
    {
        actions.push(Action::Rebase(rebase::Action::Move { from: i, to: i - 1 }));
    }
    if arrow(ui, false, i + 1 < len)
        .on_hover_text(tr!("rebase-down-hint"))
        .clicked()
    {
        actions.push(Action::Rebase(rebase::Action::Move { from: i, to: i + 2 }));
    }
    let mut chosen = row.action;
    egui::ComboBox::from_id_salt(("rebase-action", row.commit))
        .width(78.0)
        .selected_text(rebase::action_name(row.action))
        .show_ui(ui, |ui| {
            for action in rebase::ACTIONS {
                ui.selectable_value(&mut chosen, action, rebase::action_name(action))
                    .on_hover_text(rebase::action_hint(action));
            }
        });
    if chosen != row.action {
        actions.push(Action::Rebase(rebase::Action::Choose(i, chosen)));
    }
    let dropped = row.action == RebaseAction::Drop;
    let mut id = RichText::new(row.commit.short()).monospace();
    if dropped {
        id = id.strikethrough().weak();
    }
    ui.label(id);
    if row.action.is_combine() {
        // An elbow towards the commit above, which it joins.
        let (rect, _) = ui.allocate_exact_size(vec2(12.0, STEP_HEIGHT), Sense::hover());
        let stroke = egui::Stroke::new(1.5, ui.visuals().weak_text_color());
        let x = rect.left() + 3.0;
        ui.painter()
            .vline(x, rect.top() + 2.0..=rect.center().y, stroke);
        ui.painter()
            .hline(x..=rect.right(), rect.center().y, stroke);
    }
    let new_message = (editor.message_allowed(i) && !row.message.trim().is_empty())
        .then(|| row.message.lines().next().unwrap_or_default());
    let mut text = RichText::new(new_message.unwrap_or(&row.summary));
    if new_message.is_some() {
        text = text.italics();
    }
    if dropped {
        text = text.strikethrough().weak();
    }
    let badge_width = if row.published { 80.0 } else { 0.0 };
    ui.allocate_ui_with_layout(
        vec2((ui.available_width() - badge_width).max(20.0), STEP_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| ui.add(egui::Label::new(text).truncate()),
    );
    if row.published {
        ui.label(
            RichText::new(tr!("rebase-published"))
                .small()
                .color(ui.visuals().warn_fg_color),
        )
        .on_hover_text(tr!("rebase-published-hint"));
    }
}

/// The commits the plan produces, oldest first.
fn result(ui: &mut Ui, editor: &Editor, height: f32) {
    let preview = editor.preview();
    if preview.is_empty() {
        ui.weak(tr!("rebase-result-empty"));
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("rebase-result")
        .max_height(height)
        .auto_shrink([false, true])
        .show_rows(ui, RESULT_HEIGHT, preview.len(), |ui, range| {
            for i in range {
                let commit = &preview[i];
                ui.horizontal(|ui| {
                    ui.set_height(RESULT_HEIGHT);
                    ui.weak(format!("{}.", i + 1));
                    let mut text = RichText::new(&commit.summary);
                    if commit.source == MessageSource::New {
                        text = text.italics();
                    }
                    let ids: Vec<String> = commit.sources.iter().map(|id| id.short()).collect();
                    let mut detail = tr!("rebase-from", ids = ids.join(" + "));
                    match commit.source {
                        MessageSource::Combined => {
                            detail.push_str(&format!(" · {}", tr!("rebase-source-combined")));
                        }
                        MessageSource::New => {
                            detail.push_str(&format!(" · {}", tr!("rebase-source-new")));
                        }
                        MessageSource::Original => {}
                    }
                    let response = ui.add(egui::Label::new(text).truncate());
                    response.on_hover_text(&detail);
                    if commit.sources.len() > 1 || commit.source != MessageSource::Original {
                        ui.add(egui::Label::new(RichText::new(detail).small().weak()).truncate());
                    }
                });
            }
        });
}

/// The message of the selected step, where the plan takes one.
fn message_editor(ui: &mut Ui, editor: &Editor, actions: &mut Vec<Action>) {
    let Some(i) = editor.selected.filter(|&i| editor.message_allowed(i)) else {
        ui.weak(tr!("rebase-message-none"));
        return;
    };
    let row = &editor.rows[i];
    let label = if row.action == RebaseAction::Reword {
        tr!("rebase-message-reword", id = row.commit.short())
    } else {
        // The group: back to its pick, through squash/fixup and drop steps.
        let squashed = editor.rows[..=i]
            .iter()
            .rev()
            .take_while(|r| r.action.is_combine() || r.action == RebaseAction::Drop)
            .any(|r| r.action == RebaseAction::Squash);
        let default = if squashed {
            tr!("rebase-message-default-squash")
        } else {
            tr!("rebase-message-default-fixup")
        };
        tr!("rebase-message-group", default = default)
    };
    ui.label(label);
    let mut text = row.message.clone();
    let response = ui.add(
        egui::TextEdit::multiline(&mut text)
            .id_salt(("rebase-message", row.commit))
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    );
    if response.changed() {
        actions.push(Action::Rebase(rebase::Action::Message(i, text)));
    }
}

/// Rewriting published commits: the engine's reason names them.
fn confirm_dialog(ctx: &egui::Context, reason: &str, actions: &mut Vec<Action>) {
    let modal = egui::Modal::new(egui::Id::unique("rebase-confirm")).show(ctx, |ui| {
        ui.set_max_width(520.0);
        ui.heading(RichText::new(tr!("rebase-confirm-heading")).color(ui.visuals().error_fg_color));
        ui.add_space(6.0);
        ui.add(egui::Label::new(tr!("rebase-confirm-text")).wrap());
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let button = egui::Button::new(
                RichText::new(tr!("rebase-confirm-button")).color(ui.visuals().strong_text_color()),
            )
            .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
            if ui.add(button).clicked() {
                actions.push(Action::Rebase(rebase::Action::Confirm));
            }
            if ui.button(tr!("rebase-confirm-back")).clicked() {
                actions.push(Action::Rebase(rebase::Action::CancelConfirm));
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::Rebase(rebase::Action::CancelConfirm));
    }
}

/// A small up or down triangle button (the fonts have no arrow glyphs).
fn arrow(ui: &mut Ui, up: bool, enabled: bool) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(12.0, STEP_HEIGHT), sense);
    let color = if !enabled {
        ui.visuals().widgets.noninteractive.weak_bg_fill
    } else if response.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let c = rect.center();
    let (tip, base) = if up { (-4.0, 3.0) } else { (4.0, -3.0) };
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(c.x, c.y + tip),
            egui::pos2(c.x + 4.5, c.y + base),
            egui::pos2(c.x - 4.5, c.y + base),
        ],
        color,
        egui::Stroke::NONE,
    ));
    response
}
