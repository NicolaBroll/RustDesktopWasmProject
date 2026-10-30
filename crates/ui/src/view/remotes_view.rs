use super::*;

/// A remote, at the top of its branches in the "Remotes" section: caret, name (in the warning
/// color if its URL holds credentials), labels, how many branches, and "…" with its menu. The URL
/// is in the tooltip. Returns whether it was clicked (to open or close its branches).
pub(super) fn remote_row(
    ui: &mut Ui,
    remote: &RemoteListEntry,
    open: bool,
    count: usize,
    can_write: bool,
    actions: &mut Vec<Action>,
) -> bool {
    let tooltip = remote.tooltip();
    let response = row_response(ui, ui.make_persistent_id(("remote", &remote.name)));
    ui.add_space(6.0);
    caret(ui, open);
    ui.label(icons::text(icons::CLOUD, 13.0).weak());
    let name = RichText::new(&remote.name).strong();
    let name = if remote.credentials {
        name.color(ui.visuals().warn_fg_color)
    } else {
        name
    };
    ui.add(egui::Label::new(name).truncate().selectable(false));
    for (badge, hint) in remote.badges() {
        let text = RichText::new(badge)
            .small()
            .color(ui.visuals().strong_text_color())
            .background_color(ui.visuals().selection.bg_fill.gamma_multiply(0.6));
        ui.label(text).on_hover_text(hint);
    }
    ui.label(RichText::new(count.to_string()).small().weak());
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let more = egui::Button::new(icons::text(icons::MORE, 14.0)).frame(false);
        egui::containers::menu::MenuButton::from_button(more)
            .ui(ui, |ui| remote_menu(ui, remote, can_write, actions))
            .0
            .on_hover_text(tr!("remotes-menu-hint"));
    });
    let clicked = response.clicked();
    let response = response.on_hover_text(tr!("remotes-row-hint", tooltip = &tooltip));
    response.context_menu(|ui| remote_menu(ui, remote, can_write, actions));
    clicked
}

/// The menu of a remote: fetch and removal.
pub(super) fn remote_menu(
    ui: &mut Ui,
    remote: &RemoteListEntry,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let name = &remote.name;
    let button = ui
        .add_enabled(can_write, egui::Button::new(tr!("remotes-fetch-button")))
        .on_hover_text(tr!("remotes-fetch-button-hint", name = name))
        .on_disabled_hover_text(wait());
    if button.clicked() {
        actions.push(Action::Fetch(Some(name.clone())));
        ui.close();
    }
    ui.separator();
    let color = if can_write {
        ui.visuals().error_fg_color
    } else {
        ui.visuals().weak_text_color()
    };
    let button = ui
        .add_enabled(
            can_write,
            egui::Button::new(RichText::new(tr!("remotes-remove-button")).color(color)),
        )
        .on_hover_text(tr!("remotes-remove-button-hint"))
        .on_disabled_hover_text(wait());
    if button.clicked() {
        actions.push(Action::RemoveRemote(name.clone()));
        ui.close();
    }
}

/// The open remotes dialog: add or removal confirmation. `progress`: the latest
/// progress of the fetch of the add in progress.
pub(crate) fn remote_dialog(
    ctx: &egui::Context,
    dialog: &remotes::Dialog,
    progress: Option<&Progress>,
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("remotes-dialog")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        match dialog {
            remotes::Dialog::Add(add) => add_remote_dialog(ui, add, progress, actions),
            remotes::Dialog::Remove(remove) => {
                let discard::Phase::Confirm { reason } = &remove.phase else {
                    return;
                };
                remove_remote_dialog(ui, remove, reason, actions);
            }
            remotes::Dialog::DeleteBranch(delete) => {
                let discard::Phase::Confirm { reason } = &delete.phase else {
                    return;
                };
                delete_branch_dialog(ui, delete, reason, actions);
            }
        }
    });
    if modal.should_close() {
        actions.push(Action::CloseRemoteDialog);
    }
}

pub(super) fn add_remote_dialog(
    ui: &mut Ui,
    dialog: &AddDialog,
    progress: Option<&Progress>,
    actions: &mut Vec<Action>,
) {
    ui.heading(tr!("remotes-add-title"));
    ui.add_space(6.0);
    ui.add(egui::Label::new(tr!("remotes-add-explanation")).wrap());
    ui.add_space(8.0);
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let mut fields: AddFields = dialog.fields.clone();
    let mut changed = false;
    egui::Grid::new("remote-fields")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label(tr!("remotes-add-name"));
            let name = ui.add(
                egui::TextEdit::singleline(&mut fields.name)
                    .id(ui.make_persistent_id("remote-name"))
                    .hint_text(tr!("remotes-add-name-hint"))
                    .desired_width(320.0)
                    .interactive(dialog.running.is_none()),
            );
            if dialog.running.is_none() && ui.memory(|m| m.focused().is_none()) {
                name.request_focus();
            }
            changed |= name.changed();
            ui.end_row();
            ui.label(tr!("remotes-add-url"));
            if dialog.url_masked() {
                // During the submit, or after "Hide": the masked URL, read-only.
                let mut shown = dialog.shown_url();
                ui.add(
                    egui::TextEdit::singleline(&mut shown)
                        .id(ui.make_persistent_id("remote-url-masked"))
                        .desired_width(320.0)
                        .interactive(false),
                );
            } else {
                let url = ui.add(
                    egui::TextEdit::singleline(&mut fields.url)
                        .id(ui.make_persistent_id("remote-url"))
                        .hint_text(tr!("remotes-add-url-hint"))
                        .desired_width(320.0)
                        .interactive(dialog.running.is_none()),
                );
                changed |= url.changed();
            }
            ui.end_row();
        });
    if let Some(warning) = dialog.credentials_warning() {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(warning).color(ui.visuals().warn_fg_color)).wrap());
        if dialog.running.is_none() {
            let (label, hint) = if dialog.hide_url {
                (tr!("remotes-show-url"), tr!("remotes-show-url-hint"))
            } else {
                (
                    tr!("remotes-hide-credentials"),
                    tr!("remotes-hide-credentials-hint"),
                )
            };
            if ui.small_button(label).on_hover_text(hint).clicked() {
                actions.push(Action::HideRemoteUrl(!dialog.hide_url));
            }
        }
    }
    ui.add_space(4.0);
    let check = ui
        .add_enabled(
            dialog.running.is_none(),
            egui::Checkbox::new(&mut fields.fetch, tr!("remotes-add-fetch")),
        )
        .on_hover_text(tr!("remotes-add-fetch-hint"));
    changed |= check.changed();
    if changed {
        actions.push(Action::AddRemoteFields(fields));
    }
    if let Some(error) = &dialog.error {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap());
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let clicked = ui
            .add_enabled(
                dialog.can_submit(),
                egui::Button::new(tr!("remotes-add-submit")),
            )
            .clicked();
        if (clicked || enter) && dialog.can_submit() {
            actions.push(Action::SubmitAddRemote);
        }
        if ui
            .add_enabled(
                dialog.running.is_none(),
                egui::Button::new(tr!("remotes-close")),
            )
            .clicked()
        {
            actions.push(Action::CloseRemoteDialog);
        }
        if dialog.running.is_some() {
            ui.spinner();
            if !dialog.fetching() {
                ui.label(tr!("remotes-adding"));
            }
        }
    });
    if let Some(running) = dialog.running.filter(|_| dialog.fetching()) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (text, fraction) = match progress {
                Some(p) => fetch::describe(p),
                None => (tr!("remotes-connecting"), None),
            };
            ui.label(text);
            if let Some(fraction) = fraction {
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(120.0)
                        .show_percentage(),
                );
            }
            let stop = ui
                .add_enabled(
                    !running.cancelling,
                    egui::Button::new(tr!("remotes-cancel")),
                )
                .on_hover_text(tr!("remotes-cancel-hint"))
                .on_disabled_hover_text(tr!("remotes-cancelling"));
            if stop.clicked() {
                actions.push(Action::CancelAddRemote);
            }
        });
    }
}

/// Confirmation of the deletion of a branch or a tag from the remote: it disappears for everyone.
pub(super) fn delete_branch_dialog(
    ui: &mut Ui,
    delete: &DeleteBranchDialog,
    reason: &str,
    actions: &mut Vec<Action>,
) {
    let (name, remote) = (&delete.branch, &delete.remote);
    let (title, explanation, undo, confirm) = if delete.tag {
        (
            tr!("remotes-delete-tag-title", name = name, remote = remote),
            tr!(
                "remotes-delete-tag-explanation",
                name = name,
                remote = remote
            ),
            tr!("remotes-delete-tag-undo"),
            Action::ConfirmDeleteRemoteTag,
        )
    } else {
        (
            tr!("remotes-delete-branch-title", name = delete.full_name()),
            tr!(
                "remotes-delete-branch-explanation",
                branch = name,
                remote = remote
            ),
            tr!("remotes-delete-branch-undo"),
            Action::ConfirmDeleteRemoteBranch,
        )
    };
    ui.heading(RichText::new(title).color(ui.visuals().error_fg_color));
    ui.add_space(6.0);
    ui.add(egui::Label::new(explanation).wrap());
    ui.add_space(6.0);
    ui.strong(tr!("remotes-remove-what"));
    ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
    ui.add_space(6.0);
    ui.add(egui::Label::new(undo).wrap());
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = egui::Button::new(
            RichText::new(tr!("remotes-delete-branch-confirm"))
                .color(ui.visuals().strong_text_color()),
        )
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
        if ui.add_enabled(delete.expected.is_some(), button).clicked() {
            actions.push(confirm);
        }
        if ui.button(tr!("remotes-delete-branch-cancel")).clicked() {
            actions.push(Action::CloseRemoteDialog);
        }
    });
}

pub(super) fn remove_remote_dialog(
    ui: &mut Ui,
    remove: &RemoveDialog,
    reason: &str,
    actions: &mut Vec<Action>,
) {
    ui.heading(
        RichText::new(tr!("remotes-remove-title", name = &remove.name))
            .color(ui.visuals().error_fg_color),
    );
    ui.add_space(6.0);
    ui.add(egui::Label::new(tr!("remotes-remove-explanation")).wrap());
    ui.add_space(6.0);
    ui.strong(tr!("remotes-remove-what"));
    ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
    ui.add_space(6.0);
    ui.add(egui::Label::new(tr!("remotes-remove-undo")).wrap());
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = egui::Button::new(
            RichText::new(tr!("remotes-remove-confirm")).color(ui.visuals().strong_text_color()),
        )
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
        if ui.add(button).clicked() {
            actions.push(Action::ConfirmRemoveRemote);
        }
        if ui.button(tr!("remotes-remove-cancel")).clicked() {
            actions.push(Action::CloseRemoteDialog);
        }
    });
}
