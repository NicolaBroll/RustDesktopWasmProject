//! The "Open repository" dialog where the path is typed (U66): File → "Open by path…", and on
//! its own when the system's folder dialog cannot be shown.

use super::*;
use crate::tabs::{self, PathDialog};

/// The open dialog, modal. Enter opens the path typed as "Open repository…" opens a folder
/// chosen (a folder that is missing or not a repository is refused the same way); Esc closes.
pub(crate) fn open_path_dialog(
    ctx: &egui::Context,
    dialog: &PathDialog,
    actions: &mut Vec<Action>,
) {
    let push = |actions: &mut Vec<Action>, a| actions.push(Action::Tabs(a));
    let modal = egui::Modal::new(egui::Id::unique("open-path-dialog")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.heading(tr!("tabs-open-path-title"));
        ui.add_space(6.0);
        if let Some(note) = &dialog.note {
            let text = RichText::new(note).color(ui.visuals().warn_fg_color);
            ui.add(egui::Label::new(text).wrap());
            ui.add_space(6.0);
        }
        ui.add(egui::Label::new(tr!("tabs-open-path-explanation")).wrap());
        ui.add_space(8.0);
        let enter = default_enter(ui);
        let mut path = dialog.path.clone();
        let field = ui.add(
            egui::TextEdit::singleline(&mut path)
                .id(egui::Id::unique("open-path-field"))
                .hint_text(tr!("tabs-open-path-hint"))
                .desired_width(460.0),
        );
        if ui.memory(|m| m.focused().is_none()) {
            field.request_focus();
        }
        if field.changed() {
            push(actions, tabs::Action::PathText(path));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let target = dialog.target();
            let open = ui.add_enabled(
                target.is_some(),
                primary_button(ui, tr!("tabs-open-path-open"), target.is_some()),
            );
            if let Some(target) = target
                && (open.clicked() || enter)
            {
                push(actions, tabs::Action::Open(target.to_owned()));
            }
            if ui.add(cancel_button()).clicked() {
                push(actions, tabs::Action::ClosePath);
            }
        });
    });
    // Escape only: a click outside must not throw away what was typed.
    if !modal.backdrop_response.clicked() && modal.should_close() {
        push(actions, tabs::Action::ClosePath);
    }
}
