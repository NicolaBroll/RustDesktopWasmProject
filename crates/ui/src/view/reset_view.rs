//! The reset dialog (S45): the three modes with what each one does, the plan read from the engine
//! (files a `hard` overwrites, commits already published) and "Reset".

use super::*;
use crate::reset::{self, Mode, Phase, ResetDialog};
use project_engine::ResetPreviewResult;

/// Maximum height of the list of overwritten files; beyond it, it scrolls.
const FILES_HEIGHT: f32 = 160.0;

/// "Reset “branch” to here…" of the commit menu: off with a write or an open dialog, on a branch
/// without commits and during an operation in progress (the engine refuses those too).
pub(super) fn reset_item(
    ui: &mut Ui,
    repo: &RepoView,
    commit: &Commit,
    what: &str,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    ui.separator();
    let (label, blocked) = match &repo.head {
        Head::Branch { name, target } => (
            tr!("reset-menu-item", branch = name),
            target
                .is_none()
                .then(|| tr!("reset-menu-unborn", branch = name)),
        ),
        Head::Detached(_) => (tr!("reset-menu-item-detached"), None),
    };
    let blocked = blocked.or_else(|| {
        repo.status
            .as_ref()
            .is_some_and(|s| s.operation.is_some())
            .then(|| tr!("reset-menu-operation"))
    });
    let button = ui
        .add_enabled(can_write && blocked.is_none(), egui::Button::new(label))
        .on_hover_text(tr!("reset-menu-hint", id = commit.id.short()))
        .on_disabled_hover_text(blocked.unwrap_or_else(wait));
    if button.clicked() {
        actions.push(Action::Reset(reset::Action::Open {
            target: commit.id,
            what: what.to_owned(),
        }));
        ui.close();
    }
}

pub(crate) fn reset_dialog(ctx: &egui::Context, dialog: &ResetDialog, actions: &mut Vec<Action>) {
    let sending = matches!(dialog.phase, Phase::Sending(_));
    let modal = egui::Modal::new(egui::Id::unique("reset-dialog")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.heading(tr!(
            "reset-title",
            moving = dialog.moving(),
            what = &dialog.what
        ));
        ui.add_space(6.0);
        ui.add_enabled_ui(!sending, |ui| {
            for mode in Mode::ALL {
                let name = RichText::new(mode.param()).strong();
                if ui.radio(dialog.mode == mode, name).clicked() && dialog.mode != mode {
                    actions.push(Action::Reset(reset::Action::Mode(mode)));
                }
                ui.indent(mode.param(), |ui| {
                    ui.add(egui::Label::new(RichText::new(mode.hint()).weak()).wrap());
                });
            }
        });
        ui.add_space(8.0);
        if let Some(error) = &dialog.error {
            error_text(ui, error);
            ui.add_space(4.0);
        }
        match &dialog.phase {
            Phase::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr!("reset-loading"));
                });
            }
            Phase::Refused(reason) => error_text(ui, &tr!("reset-not-possible", error = reason)),
            Phase::Ready(preview) | Phase::Sending(preview) => plan(ui, dialog, preview),
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let hard = dialog.mode == Mode::Hard
                && matches!(&dialog.phase, Phase::Ready(p) | Phase::Sending(p) if !p.files.is_empty());
            let mut button = egui::Button::new(
                RichText::new(tr!("reset-button", mode = dialog.mode.param()))
                    .color(ui.visuals().strong_text_color()),
            );
            if hard {
                button = button.fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
            }
            if ui.add_enabled(dialog.can_submit(), button).clicked() {
                actions.push(Action::Reset(reset::Action::Submit));
            }
            if ui
                .add_enabled(!sending, egui::Button::new(tr!("reset-cancel")))
                .clicked()
            {
                actions.push(Action::Reset(reset::Action::Close));
            }
            if sending {
                ui.spinner();
                ui.label(tr!("reset-sending"));
            }
        });
    });
    if !sending && modal.should_close() {
        actions.push(Action::Reset(reset::Action::Close));
    }
}

/// What the chosen mode does here: the move, the files overwritten, the warnings.
fn plan(ui: &mut Ui, dialog: &ResetDialog, preview: &ResetPreviewResult) {
    let body = if preview.before == preview.after {
        tr!(
            "reset-plan-same",
            moving = dialog.moving(),
            id = preview.after.short()
        )
    } else {
        tr!(
            "reset-plan-move",
            moving = dialog.moving(),
            before = preview.before.short(),
            after = preview.after.short()
        )
    };
    ui.add(egui::Label::new(body).wrap());
    if dialog.mode == Mode::Hard {
        ui.add_space(6.0);
        let n = preview.files.len();
        if n == 0 {
            ui.label(tr!("reset-hard-clean"));
        } else {
            ui.add(egui::Label::new(tr!("reset-hard-files", n = n)).wrap());
            let row = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::vertical()
                .id_salt("reset-files")
                .max_height(FILES_HEIGHT)
                .auto_shrink([false, true])
                .show_rows(ui, row, n, |ui, range| {
                    for path in &preview.files[range] {
                        ui.monospace(path.to_string_lossy());
                    }
                });
            ui.add(egui::Label::new(RichText::new(tr!("reset-hard-backup")).weak()).wrap());
        }
        ui.add(egui::Label::new(RichText::new(tr!("reset-hard-untracked")).weak()).wrap());
    }
    for warning in &preview.warnings {
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                RichText::new(tr!("reset-warning", warning = warning.text()))
                    .color(ui.visuals().warn_fg_color),
            )
            .wrap(),
        );
    }
}

fn error_text(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(ui.visuals().error_fg_color)).wrap());
}
