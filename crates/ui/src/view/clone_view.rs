//! The "Clone…" and "New repository…" dialog (S46): URL (clone only), destination folder with
//! "Browse…", optional branch and depth; during the clone, git's progress and "Cancel".

use super::*;
use crate::clone::{self, CloneDialog, Kind};

/// "Clone…" and "New repository…", for the File menu and the empty screen.
pub(super) fn create_buttons(ui: &mut Ui, actions: &mut Vec<Action>) {
    if ui
        .button(tr!("clone-menu-clone"))
        .on_hover_text(tr!("clone-menu-clone-hint"))
        .clicked()
    {
        actions.push(Action::Clone(clone::Action::Open(Kind::Clone)));
        ui.close();
    }
    if ui
        .button(tr!("clone-menu-init"))
        .on_hover_text(tr!("clone-menu-init-hint"))
        .clicked()
    {
        actions.push(Action::Clone(clone::Action::Open(Kind::Init)));
        ui.close();
    }
}

/// The open dialog. `progress`: the latest progress of the clone in progress; `can_pick`: the
/// platform has a folder dialog ("Browse…").
pub(crate) fn clone_dialog(
    ctx: &egui::Context,
    dialog: &CloneDialog,
    progress: Option<&Progress>,
    can_pick: bool,
    actions: &mut Vec<Action>,
) {
    let running = dialog.running.is_some();
    let modal = egui::Modal::new(egui::Id::unique("clone-dialog")).show(ctx, |ui| {
        ui.set_max_width(520.0);
        let (title, explanation) = match dialog.kind {
            Kind::Clone => (tr!("clone-title"), tr!("clone-explanation")),
            Kind::Init => (tr!("clone-init-title"), tr!("clone-init-explanation")),
        };
        ui.heading(title);
        ui.add_space(6.0);
        ui.add(egui::Label::new(explanation).wrap());
        ui.add_space(8.0);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let mut fields = dialog.fields.clone();
        let mut changed = false;
        egui::Grid::new("clone-fields")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                if dialog.kind == Kind::Clone {
                    ui.label(tr!("clone-url"));
                    let url = ui.add(
                        egui::TextEdit::singleline(&mut fields.url)
                            .id(ui.make_persistent_id("clone-url"))
                            .hint_text(tr!("clone-url-hint"))
                            .desired_width(380.0)
                            .interactive(!running),
                    );
                    if !running && ui.memory(|m| m.focused().is_none()) {
                        url.request_focus();
                    }
                    changed |= url.changed();
                    ui.end_row();
                }
                ui.label(tr!("clone-path"));
                ui.horizontal(|ui| {
                    let width = if can_pick { 290.0 } else { 380.0 };
                    let path = ui.add(
                        egui::TextEdit::singleline(&mut fields.path)
                            .id(ui.make_persistent_id("clone-path"))
                            .hint_text(tr!("clone-path-hint"))
                            .desired_width(width)
                            .interactive(!running),
                    );
                    if dialog.kind == Kind::Init && !running && ui.memory(|m| m.focused().is_none())
                    {
                        path.request_focus();
                    }
                    changed |= path.changed();
                    if can_pick {
                        let hint = match dialog.kind {
                            Kind::Clone => tr!("clone-browse-hint"),
                            Kind::Init => tr!("clone-init-browse-hint"),
                        };
                        let browse = ui
                            .add_enabled(!running, egui::Button::new(tr!("clone-browse")))
                            .on_hover_text(hint);
                        if browse.clicked() {
                            actions.push(Action::Clone(clone::Action::Browse));
                        }
                    }
                });
                ui.end_row();
                ui.label(tr!("clone-branch"));
                let hint = match dialog.kind {
                    Kind::Clone => tr!("clone-branch-hint"),
                    Kind::Init => tr!("clone-init-branch-hint"),
                };
                let branch = ui.add(
                    egui::TextEdit::singleline(&mut fields.branch)
                        .id(ui.make_persistent_id("clone-branch"))
                        .hint_text(hint)
                        .desired_width(380.0)
                        .interactive(!running),
                );
                changed |= branch.changed();
                ui.end_row();
                if dialog.kind == Kind::Clone {
                    ui.label(tr!("clone-depth"));
                    let depth = ui
                        .add(
                            egui::TextEdit::singleline(&mut fields.depth)
                                .id(ui.make_persistent_id("clone-depth"))
                                .hint_text(tr!("clone-depth-hint"))
                                .desired_width(120.0)
                                .interactive(!running),
                        )
                        .on_hover_text(tr!("clone-depth-tooltip"));
                    changed |= depth.changed();
                    ui.end_row();
                }
            });
        if changed {
            actions.push(Action::Clone(clone::Action::Fields(fields)));
        }
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(tr!("clone-path-rule")).weak()).wrap());
        if let Some(problem) = dialog.problem() {
            ui.add_space(4.0);
            error_label(ui, &problem);
        }
        if dialog.cancelled {
            ui.add_space(4.0);
            ui.add(egui::Label::new(tr!("clone-cancelled")).wrap());
        }
        if let Some(failure) = &dialog.failure {
            ui.add_space(6.0);
            ui.label(
                RichText::new(&failure.title)
                    .strong()
                    .color(ui.visuals().error_fg_color),
            );
            ui.add(egui::Label::new(&failure.explanation).wrap());
            git_text(ui, failure.git.as_deref());
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let label = match dialog.kind {
                Kind::Clone => tr!("clone-submit"),
                Kind::Init => tr!("clone-init-submit"),
            };
            let clicked = ui
                .add_enabled(dialog.can_submit(), egui::Button::new(label))
                .clicked();
            if (clicked || enter) && dialog.can_submit() {
                actions.push(Action::Clone(clone::Action::Submit));
            }
            match dialog.running {
                Some(r) => {
                    let stop = ui
                        .add_enabled(
                            dialog.kind == Kind::Clone && !r.cancelling,
                            egui::Button::new(tr!("clone-cancel")),
                        )
                        .on_hover_text(tr!("clone-cancel-hint"))
                        .on_disabled_hover_text(tr!("clone-cancelling"));
                    if stop.clicked() {
                        actions.push(Action::Clone(clone::Action::Cancel));
                    }
                }
                None => {
                    if ui.button(tr!("clone-close")).clicked() {
                        actions.push(Action::Clone(clone::Action::Close));
                    }
                }
            }
        });
        if let Some(r) = dialog.running {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spinner();
                let (text, fraction) = match (dialog.kind, progress) {
                    _ if r.cancelling => (tr!("clone-cancelling"), None),
                    (Kind::Clone, Some(p)) => fetch::describe(p),
                    (Kind::Clone, None) => (tr!("clone-connecting"), None),
                    (Kind::Init, _) => (tr!("clone-init-running"), None),
                };
                let label = ui.label(text);
                if let Some(p) = progress {
                    label.on_hover_text(&p.text);
                }
                if let Some(fraction) = fraction {
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_width(160.0)
                            .show_percentage(),
                    );
                }
            });
        }
    });
    if !running && modal.should_close() {
        actions.push(Action::Clone(clone::Action::Close));
    }
}

fn error_label(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(ui.visuals().error_fg_color)).wrap());
}
