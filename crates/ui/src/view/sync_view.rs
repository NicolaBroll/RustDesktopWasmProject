use super::*;
use crate::{fetch, stash, sync};

/// The dialog of a failed or refused operation: what happened, what to do, git's text (with
/// "Copy message") and "Stash…" for a refusal because of local changes. Modal like the
/// confirmations: Esc closes it, the keyboard starts on "Stash…" or "Close" (Enter presses
/// it); `dismiss` closes it.
pub(crate) fn failure_dialog(
    ctx: &egui::Context,
    failure: &Failure,
    dismiss: &Action,
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("failure")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading(&failure.title);
        ui.add_space(6.0);
        ui.add(egui::Label::new(&failure.explanation).wrap());
        let git = failure.git.as_deref().filter(|g| !g.trim().is_empty());
        git_text(ui, git);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            // Enter presses the focused button: the stash when offered, otherwise "Close".
            let stash = failure.stash.then(|| {
                ui.button(tr!("stage-stash-button"))
                    .on_hover_text(tr!("common-stash-button-help"))
            });
            let close = ui.button(tr!("sync-close"));
            if ui.memory(|m| m.focused().is_none()) {
                stash.as_ref().unwrap_or(&close).request_focus();
            }
            if stash.is_some_and(|s| s.clicked()) {
                actions.push(dismiss.clone());
                actions.push(Action::Stash(stash::Action::Open));
            }
            if close.clicked() {
                actions.push(dismiss.clone());
            }
            if let Some(git) = git
                && ui
                    .button(tr!("common-copy-message"))
                    .on_hover_text(tr!("common-copy-message-help"))
                    .clicked()
            {
                ui.ctx().copy_text(format!(
                    "{}\n{}\n\n{}",
                    failure.title,
                    failure.explanation,
                    git.trim_end()
                ));
            }
        });
    });
    if modal.should_close() {
        actions.push(dismiss.clone());
    }
}

/// The pull and push window: rejection or force confirmation (a failure is drawn by
/// [`failure_dialog`], one at a time with those of the other areas). `writing`: a write is in
/// progress; `can_write`: no write and no dialog, the buttons that retry are enabled.
pub(crate) fn sync_window(
    ctx: &egui::Context,
    window: &Window,
    writing: bool,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    match window {
        Window::Failure(_) => {}
        Window::Rejected(rejected) => rejected_window(ctx, rejected, can_write, actions),
        Window::ConfirmForce(confirm) => confirm_force(ctx, confirm, writing, actions),
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
                        actions.push(Action::Sync(sync::Action::Pull(
                            rejected.branch.clone(),
                            None,
                        )));
                    }
                }
                if rejected.stale() {
                    let fetch = ui
                        .add_enabled(can_write, egui::Button::new(tr!("sync-fetch-button")))
                        .on_hover_text(tr!("sync-fetch-button-hint"))
                        .on_disabled_hover_text(wait());
                    if fetch.clicked() {
                        actions.push(Action::Sync(sync::Action::Dismiss));
                        actions.push(Action::Fetch(fetch::Action::Fetch(None)));
                    }
                }
                if ui.button(tr!("sync-close")).clicked() {
                    actions.push(Action::Sync(sync::Action::Dismiss));
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
                        actions.push(Action::Sync(sync::Action::ForcePush));
                    }
                }
            });
        });
    if !open {
        actions.push(Action::Sync(sync::Action::Dismiss));
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
                actions.push(Action::Sync(sync::Action::ConfirmForcePush));
            }
            if ui.button(tr!("sync-force-confirm-cancel")).clicked() {
                actions.push(Action::Sync(sync::Action::Dismiss));
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::Sync(sync::Action::Dismiss));
    }
}
