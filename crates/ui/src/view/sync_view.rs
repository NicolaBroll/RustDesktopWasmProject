use super::*;

/// The window of a failed operation: what happened, what to do, git's text. It does not
/// block the rest of the window; `dismiss` closes it.
pub(crate) fn failure_window(
    ctx: &egui::Context,
    id: &str,
    failure: &Failure,
    dismiss: Action,
    actions: &mut Vec<Action>,
) {
    let mut open = true;
    egui::Window::new(&failure.title)
        .id(egui::Id::unique(id))
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.add(egui::Label::new(&failure.explanation).wrap());
            git_text(ui, failure.git.as_deref());
            ui.add_space(6.0);
            if ui.button(tr!("sync-close")).clicked() {
                actions.push(dismiss.clone());
            }
        });
    if !open {
        actions.push(dismiss);
    }
}

/// The pull and push window: error, conflict, rejection or force confirmation. `writing`:
/// a write is in progress; `can_write`: no write and no dialog, the buttons that
/// retry are enabled.
pub(crate) fn sync_window(
    ctx: &egui::Context,
    window: &Window,
    writing: bool,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    match window {
        Window::Failure(failure) => {
            failure_window(ctx, "sync-error", failure, Action::DismissSync, actions);
        }
        Window::Conflict(conflict) => conflict_window(ctx, conflict, can_write, actions),
        Window::Rejected(rejected) => rejected_window(ctx, rejected, can_write, actions),
        Window::ConfirmForce(confirm) => confirm_force(ctx, confirm, writing, actions),
    }
}

/// Maximum height of the conflicted file list; beyond it scrolls.
pub(super) const CONFLICT_LIST_HEIGHT: f32 = 160.0;

/// Pull stopped on a conflict and already aborted by the engine (today only the rebase: a stopped merge stays
/// in progress and is resolved from the operation panel).
pub(super) fn conflict_window(
    ctx: &egui::Context,
    conflict: &sync::Conflict,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let mut open = true;
    let operation = conflict.operation.to_string();
    egui::Window::new(tr!("sync-conflict-title"))
        .id(egui::Id::unique("pull-conflict"))
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.add(
                egui::Label::new(tr!("sync-conflict-explanation", operation = &operation)).wrap(),
            );
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(tr!("sync-conflict-hint", other = conflict.other().name())).wrap(),
            );
            ui.add_space(6.0);
            let n = conflict.files.len();
            ui.strong(match n {
                0 => tr!("sync-conflict-no-files"),
                n => tr!("sync-conflict-files", n = n),
            });
            let row = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::vertical()
                .id_salt("conflict-file")
                .max_height(CONFLICT_LIST_HEIGHT)
                .auto_shrink([false, true])
                .show_rows(ui, row, n, |ui, range| {
                    for path in &conflict.files[range] {
                        ui.monospace(path);
                    }
                });
            ui.add_space(6.0);
            ui.collapsing(tr!("sync-engine-message"), |ui| {
                ui.add(egui::Label::new(RichText::new(&conflict.message).monospace()).wrap());
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let other = conflict.other();
                let button = ui
                    .add_enabled(
                        can_write,
                        egui::Button::new(tr!("sync-pull-with", other = other.name())),
                    )
                    .on_hover_text(tr!(
                        "sync-pull-with-hint",
                        other = other.name(),
                        operation = &operation
                    ))
                    .on_disabled_hover_text(wait());
                if button.clicked() {
                    actions.push(Action::Pull(conflict.branch.clone(), Some(other)));
                }
                if ui.button(tr!("sync-close")).clicked() {
                    actions.push(Action::DismissSync);
                }
            });
        });
    if !open {
        actions.push(Action::DismissSync);
    }
}

/// Push rejected: what happened and what to do. Not fast-forward: "Pull" and, as a second
/// choice, "Force (with lease)..."; stale lease: "Fetch".
pub(super) fn rejected_window(
    ctx: &egui::Context,
    rejected: &sync::Rejected,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let mut open = true;
    egui::Window::new(tr!("sync-rejected-title"))
        .id(egui::Id::unique("push-rejected"))
        .collapsible(false)
        .resizable(false)
        .default_width(440.0)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
        .open(&mut open)
        .show(ctx, |ui| {
            if let Some(branch) = &rejected.branch {
                ui.strong(tr!("sync-rejected-branch", branch = branch));
                ui.add_space(4.0);
            }
            ui.add(egui::Label::new(&rejected.explanation).wrap());
            git_text(ui, rejected.git.as_deref());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if rejected.non_fast_forward() {
                    let pull = ui
                        .add_enabled(can_write, egui::Button::new(tr!("sync-pull-button")))
                        .on_hover_text(tr!("sync-pull-button-hint"))
                        .on_disabled_hover_text(wait());
                    if pull.clicked() {
                        actions.push(Action::Pull(rejected.branch.clone(), None));
                    }
                }
                if rejected.stale() {
                    let fetch = ui
                        .add_enabled(can_write, egui::Button::new(tr!("sync-fetch-button")))
                        .on_hover_text(tr!("sync-fetch-button-hint"))
                        .on_disabled_hover_text(wait());
                    if fetch.clicked() {
                        actions.push(Action::DismissSync);
                        actions.push(Action::Fetch(None));
                    }
                }
                if ui.button(tr!("sync-close")).clicked() {
                    actions.push(Action::DismissSync);
                }
                if rejected.non_fast_forward() {
                    ui.separator();
                    let force = ui
                        .add_enabled(
                            can_write,
                            egui::Button::new(
                                RichText::new(tr!("sync-force-button"))
                                    .color(ui.visuals().error_fg_color),
                            )
                            .frame(false),
                        )
                        .on_hover_text(tr!("sync-force-button-hint"))
                        .on_disabled_hover_text(wait());
                    if force.clicked() {
                        actions.push(Action::ForcePush);
                    }
                }
            });
        });
    if !open {
        actions.push(Action::DismissSync);
    }
}

/// Confirmation of the forced push: the remote would lose commits that the branch does not have.
pub(super) fn confirm_force(
    ctx: &egui::Context,
    confirm: &sync::ConfirmForce,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("force-confirm")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.heading(
            RichText::new(match &confirm.branch {
                Some(b) => tr!("sync-force-confirm-title-branch", branch = b),
                None => tr!("sync-force-confirm-title"),
            })
            .color(ui.visuals().error_fg_color),
        );
        ui.add_space(6.0);
        ui.add(egui::Label::new(tr!("sync-force-confirm-explanation")).wrap());
        ui.add_space(4.0);
        ui.add(egui::Label::new(tr!("sync-force-confirm-backup")).wrap());
        if confirm.changed {
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new(tr!("sync-force-confirm-changed"))
                        .strong()
                        .color(ui.visuals().warn_fg_color),
                )
                .wrap(),
            );
        }
        ui.add_space(6.0);
        ui.strong(tr!("sync-force-confirm-lost"));
        egui::ScrollArea::vertical()
            .id_salt("force-reason")
            .max_height(160.0)
            .show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&confirm.reason).monospace()).wrap());
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let button = egui::Button::new(
                RichText::new(tr!("sync-force-confirm-button"))
                    .color(ui.visuals().strong_text_color()),
            )
            .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
            let response = ui
                .add_enabled(!writing && confirm.can_confirm(), button)
                .on_disabled_hover_text(if confirm.can_confirm() {
                    wait()
                } else {
                    tr!("sync-force-confirm-gone")
                });
            if response.clicked() {
                actions.push(Action::ConfirmForcePush);
            }
            if ui.button(tr!("sync-force-confirm-cancel")).clicked() {
                actions.push(Action::DismissSync);
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::DismissSync);
    }
}
