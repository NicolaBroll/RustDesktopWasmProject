use super::*;

/// How many conflicted files to list in the confirmation; the others become "and N more".
pub(super) const CONFIRM_LIST: usize = 8;

/// Confirmation before staging conflicted files: explains that they are marked resolved.
pub(crate) fn confirm_write(ctx: &egui::Context, write: &PendingWrite, actions: &mut Vec<Action>) {
    let modal = egui::Modal::new(egui::Id::unique("write-confirm")).show(ctx, |ui| {
        ui.set_max_width(440.0);
        let n = write.conflicts.len();
        ui.heading(tr!("confirm-resolve-title", n = n));
        ui.add_space(6.0);
        ui.add(egui::Label::new(tr!("confirm-resolve-body")).wrap());
        ui.add_space(6.0);
        ui.strong(tr!("confirm-resolve-files", n = n));
        for path in write.conflicts.iter().take(CONFIRM_LIST) {
            ui.monospace(path.to_string_lossy());
        }
        if n > CONFIRM_LIST {
            ui.weak(tr!("confirm-and-more", n = n - CONFIRM_LIST));
        }
        let others = write.paths.len() - n;
        if others > 0 {
            ui.add_space(4.0);
            ui.weak(tr!("confirm-resolve-others", n = others));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let label = tr!("confirm-resolve-button", n = n);
            if ui.button(label).clicked() {
                actions.push(Action::ConfirmWrite);
            }
            if ui.button(tr!("confirm-cancel")).clicked() {
                actions.push(Action::CancelWrite);
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::CancelWrite);
    }
}

/// Maximum height of the file list in the discard confirmation; beyond it, it scrolls.
pub(super) const DISCARD_LIST_HEIGHT: f32 = 160.0;

/// Confirmation before discarding unstaged changes: lists the files and explains how to
/// undo.
pub(crate) fn confirm_discard(ctx: &egui::Context, pending: &Discard, actions: &mut Vec<Action>) {
    let modal = egui::Modal::new(egui::Id::unique("discard-confirm")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        let n = pending.paths.len();
        ui.heading(tr!("confirm-discard-title"));
        ui.add_space(6.0);
        ui.add(egui::Label::new(tr!("confirm-discard-body")).wrap());
        ui.add_space(4.0);
        ui.add(egui::Label::new(tr!("confirm-discard-backup")).wrap());
        ui.add_space(6.0);
        ui.strong(tr!("confirm-discard-files", n = n));
        let row = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::vertical()
            .id_salt("discard-files")
            .max_height(DISCARD_LIST_HEIGHT)
            .auto_shrink([false, true])
            .show_rows(ui, row, n, |ui, range| {
                for path in &pending.paths[range] {
                    ui.monospace(path.to_string_lossy());
                }
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let label = tr!("confirm-discard-button", n = n);
            let button =
                egui::Button::new(RichText::new(label).color(ui.visuals().strong_text_color()))
                    .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
            if ui.add(button).clicked() {
                actions.push(Action::ConfirmDiscard);
            }
            if ui.button(tr!("confirm-discard-keep")).clicked() {
                actions.push(Action::CancelDiscard);
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::CancelDiscard);
    }
}

/// Confirmation before rewriting with amend a commit already on a remote branch.
pub(crate) fn confirm_amend(
    ctx: &egui::Context,
    confirm: &ConfirmAmend,
    left_behind: &[String],
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("amend-confirm")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading(tr!("confirm-amend-title"));
        ui.add_space(6.0);
        ui.add(egui::Label::new(tr!("confirm-amend-body")).wrap());
        ui.add_space(6.0);
        ui.strong(tr!("confirm-amend-reason"));
        ui.add(egui::Label::new(RichText::new(&confirm.reason).monospace()).wrap());
        if !left_behind.is_empty() {
            ui.add_space(6.0);
            ui.strong(tr!("confirm-amend-left-behind"));
            for name in left_behind.iter().take(CONFIRM_LIST) {
                ui.monospace(name);
            }
            if left_behind.len() > CONFIRM_LIST {
                ui.weak(tr!(
                    "confirm-and-more",
                    n = left_behind.len() - CONFIRM_LIST
                ));
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button(tr!("confirm-amend-button")).clicked() {
                actions.push(Action::ConfirmCommit);
            }
            if ui.button(tr!("confirm-cancel")).clicked() {
                actions.push(Action::CancelCommit);
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::CancelCommit);
    }
}
