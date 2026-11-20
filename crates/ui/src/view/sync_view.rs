use super::*;
use crate::failure::Source;
use crate::{fetch, stash, sync};

/// The dialog of a failed or refused operation: what happened, what to do, git's text (with
/// "Copy message"), "Stash…" for a refusal because of local changes and "Set name and email…"
/// for a commit git could not create without an identity. Modal like the
/// confirmations: Esc closes it, the keyboard starts on "Stash…" or "Close" (Enter presses
/// it). `source`: the area the failure comes from, which closes it; "Stash…" hands that area's
/// refused command to the stash dialog ("Stash and …", U68).
pub(crate) fn failure_dialog(
    ctx: &egui::Context,
    failure: &Failure,
    source: Source,
    actions: &mut Vec<Action>,
) {
    let dismiss = source.dismiss();
    let modal = egui::Modal::new(egui::Id::unique("failure")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading(&failure.title);
        ui.add_space(6.0);
        ui.add(egui::Label::new(&failure.explanation).wrap());
        let git = failure.git.as_deref().filter(|g| !g.trim().is_empty());
        git_text(ui, git);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            // Enter presses the focused button: the stash or the identity when offered,
            // otherwise "Close".
            let stash = failure.stash.then(|| {
                ui.button(tr!("stage-stash-button"))
                    .on_hover_text(tr!("common-stash-button-help"))
            });
            // The settings open over what is under this dialog, which closes: afterwards the
            // user is back where the command was sent from.
            let mut opened = Vec::new();
            let identity = failure
                .identity
                .then(|| set_identity_button(ui, true, &mut opened));
            if !opened.is_empty() {
                actions.push(dismiss.clone());
                actions.append(&mut opened);
            }
            let close = ui.button(tr!("sync-close"));
            if ui.memory(|m| m.focused().is_none()) {
                stash
                    .as_ref()
                    .or(identity.as_ref())
                    .unwrap_or(&close)
                    .request_focus();
            }
            if stash.is_some_and(|s| s.clicked()) {
                actions.push(Action::Stash(stash::Action::OpenAfter(source)));
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
        actions.push(dismiss);
    }
}

/// The pull and push dialog: the details of a rejected push or the force confirmation (a
/// failure is drawn by [`failure_dialog`], one at a time with those of the other areas).
/// `writing`: a write is in progress.
pub(crate) fn sync_window(
    ctx: &egui::Context,
    window: &Window,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    match window {
        Window::Failure(_) => {}
        Window::Details(rejected) => rejected_details(ctx, rejected, writing, actions),
        Window::ConfirmForce(confirm) => confirm_force(ctx, confirm, writing, actions),
    }
}

/// The notice of a rejected push, under the bar like the panel of the operation in progress:
/// what was rejected and what to do (wrapped, never cut), "Pull" for a non-fast-forward or
/// "Fetch" for a stale lease, "Details…" (git's text, the force) and "×". It blocks nothing.
/// `can_write`: no write and no dialog, "Pull" and "Fetch" are enabled.
pub(crate) fn rejected_notice(
    ui: &mut Ui,
    rejected: &sync::Rejected,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let warn = ui.visuals().warn_fg_color;
    egui::Frame::new()
        .fill(warn.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, warn.gamma_multiply(0.6)))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .outer_margin(egui::Margin::symmetric(0, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // The buttons first, from the right: the text wraps in what is left.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    rejected_buttons(ui, rejected, can_write, actions);
                    let wrapped =
                        egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true);
                    ui.with_layout(wrapped, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(&rejected.title).strong().color(warn));
                        ui.label(&rejected.summary);
                    });
                });
            });
        });
}

/// The buttons of the notice, from the right: "×", "Details…" and what brings the branch up
/// to date.
fn rejected_buttons(
    ui: &mut Ui,
    rejected: &sync::Rejected,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    if ui
        .button("×")
        .on_hover_text(tr!("sync-rejected-dismiss-hint"))
        .clicked()
    {
        actions.push(Action::Sync(sync::Action::DismissRejected));
    }
    if ui
        .button(tr!("sync-details-button"))
        .on_hover_text(tr!("sync-details-button-hint"))
        .clicked()
    {
        actions.push(Action::Sync(sync::Action::RejectedDetails));
    }
    if rejected.non_fast_forward() && rejected.branch.is_some() {
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
            actions.push(Action::Sync(sync::Action::DismissRejected));
            actions.push(Action::Fetch(fetch::Action::Fetch(None)));
        }
    }
}

/// "Details…" of a rejected push: the explanation in full, git's text (scrolls, never cut)
/// with "Copy message" and, for a non-fast-forward, "Force (with lease)…", which leads to
/// [`confirm_force`]. Modal: Esc closes it, the keyboard starts on "Close".
fn rejected_details(
    ctx: &egui::Context,
    rejected: &sync::Rejected,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("push-rejected")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.heading(&rejected.title);
        ui.add_space(6.0);
        ui.add(egui::Label::new(&rejected.explanation).wrap());
        let git = rejected.git.as_deref().filter(|g| !g.trim().is_empty());
        git_text(ui, git);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let close = ui.button(tr!("sync-close"));
            if ui.memory(|m| m.focused().is_none()) {
                close.request_focus();
            }
            if close.clicked() {
                actions.push(Action::Sync(sync::Action::Dismiss));
            }
            if let Some(git) = git
                && ui
                    .button(tr!("common-copy-message"))
                    .on_hover_text(tr!("common-copy-message-help"))
                    .clicked()
            {
                ui.ctx().copy_text(format!(
                    "{}\n{}\n\n{}",
                    rejected.title,
                    rejected.explanation,
                    git.trim_end()
                ));
            }
            if rejected.can_force() {
                ui.separator();
                let force = ui
                    .add_enabled(
                        !writing,
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
    if modal.should_close() {
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
            let button = danger_button(ui, tr!("sync-force-confirm-button"));
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
            if ui.add(cancel_button()).clicked() {
                actions.push(Action::Sync(sync::Action::Dismiss));
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::Sync(sync::Action::Dismiss));
    }
}
