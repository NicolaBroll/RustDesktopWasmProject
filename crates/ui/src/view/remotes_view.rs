use super::*;

/// The "Remotes" section: the configured remotes (`remote.list`), with fetch and removal for
/// each one, and "Add remote...".
pub(super) fn remotes_section(
    ui: &mut Ui,
    repo: &RepoView,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let list = repo.remotes.list();
    let title = match list {
        Some(list) => tr!("remotes-section-count", n = list.len()),
        None => tr!("remotes-section"),
    };
    egui::CollapsingHeader::new(title)
        .id_salt("remotes-section")
        .default_open(true)
        .show(ui, |ui| {
            match list {
                Some([]) => {
                    ui.weak(tr!("remotes-none"));
                }
                Some(list) => {
                    for remote in list {
                        ui.push_id(("remote", &remote.name), |ui| {
                            remote_row(ui, remote, can_write, actions);
                        });
                    }
                }
                None if repo.remotes.list_error().is_none() => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak(tr!("remotes-loading"));
                    });
                }
                None => {}
            }
            if let Some(error) = repo.remotes.list_error() {
                ui.add(
                    egui::Label::new(
                        RichText::new(tr!("remotes-list-error", error = error))
                            .color(ui.visuals().error_fg_color)
                            .small(),
                    )
                    .wrap(),
                );
            }
            let button = ui
                .add_enabled(can_write, egui::Button::new(tr!("remotes-add-button")))
                .on_hover_text(tr!("remotes-add-button-hint"))
                .on_disabled_hover_text(wait());
            if button.clicked() {
                actions.push(Action::AddRemote);
            }
        });
}

/// A row of the "Remotes" section: name, labels, how many branches, URL (masked by the
/// engine) and the menu with fetch and removal.
pub(super) fn remote_row(ui: &mut Ui, remote: &Remote, can_write: bool, actions: &mut Vec<Action>) {
    let tooltip = remote.tooltip();
    ui.horizontal(|ui| {
        let name = ui
            .add(egui::Label::new(RichText::new(&remote.name).strong()).sense(Sense::click()))
            .on_hover_text(tr!("remotes-row-hint", tooltip = &tooltip));
        name.context_menu(|ui| remote_menu(ui, remote, can_write, actions));
        for (badge, hint) in remote.badges() {
            let text = RichText::new(badge)
                .small()
                .color(ui.visuals().strong_text_color())
                .background_color(ui.visuals().selection.bg_fill.gamma_multiply(0.6));
            ui.label(text).on_hover_text(hint);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("…", |ui| remote_menu(ui, remote, can_write, actions))
                .response
                .on_hover_text(tr!("remotes-menu-hint"));
        });
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new(remote.branches_text()).small().weak());
        ui.label(RichText::new("·").small().weak());
        let url = RichText::new(remote.url_text()).small().weak();
        let url = if remote.credentials {
            url.color(ui.visuals().warn_fg_color)
        } else {
            url
        };
        ui.add(egui::Label::new(url).truncate())
            .on_hover_text(tooltip);
    });
    ui.add_space(2.0);
}

/// The menu of a remote: fetch and removal.
pub(super) fn remote_menu(
    ui: &mut Ui,
    remote: &Remote,
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
                    .interactive(!dialog.sending),
            );
            if !dialog.sending && ui.memory(|m| m.focused().is_none()) {
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
                        .interactive(!dialog.sending),
                );
                changed |= url.changed();
            }
            ui.end_row();
        });
    if let Some(warning) = dialog.credentials_warning() {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(warning).color(ui.visuals().warn_fg_color)).wrap());
        if !dialog.sending {
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
            !dialog.sending,
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
            .add_enabled(!dialog.sending, egui::Button::new(tr!("remotes-close")))
            .clicked()
        {
            actions.push(Action::CloseRemoteDialog);
        }
        if dialog.sending {
            ui.spinner();
            if !dialog.fetching {
                ui.label(tr!("remotes-adding"));
            }
        }
    });
    if dialog.sending && dialog.fetching {
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
                .add_enabled(!dialog.cancelling, egui::Button::new(tr!("remotes-cancel")))
                .on_hover_text(tr!("remotes-cancel-hint"))
                .on_disabled_hover_text(tr!("remotes-cancelling"));
            if stop.clicked() {
                actions.push(Action::CancelAddRemote);
            }
        });
    }
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
