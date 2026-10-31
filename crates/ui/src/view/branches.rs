use super::*;
use crate::branch;
use crate::tracking;

/// The open branch dialog: name (new or rename), delete confirmation,
/// refused checkout.
pub(crate) fn branch_dialog(ctx: &egui::Context, dialog: &Dialog, actions: &mut Vec<Action>) {
    let modal = egui::Modal::new(egui::Id::unique("branch-dialog")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        match dialog {
            Dialog::Name(dialog) => name_dialog(ui, dialog, actions),
            Dialog::Delete(delete) => {
                let discard::Phase::Confirm { reason } = &delete.phase else {
                    return;
                };
                delete_dialog(ui, delete, reason, actions);
            }
            Dialog::DeleteTag(delete) => {
                let discard::Phase::Confirm { reason } = &delete.phase else {
                    return;
                };
                delete_tag_dialog(ui, delete, reason, actions);
            }
            Dialog::Refused(refused) => refused_dialog(ui, refused, actions),
            Dialog::Upstream(dialog) => upstream_dialog(ui, dialog, actions),
        }
    });
    if modal.should_close() {
        actions.push(Action::Branch(branch::Action::CloseDialog));
    }
}

pub(super) fn name_dialog(ui: &mut Ui, dialog: &branch::NameDialog, actions: &mut Vec<Action>) {
    let (title, button) = match &dialog.kind {
        NameKind::Create(_) => (tr!("branch-new-title"), tr!("branch-create-button")),
        NameKind::Rename(_) => (tr!("branch-rename-title"), tr!("branch-rename-button")),
        NameKind::Tag(_) => (tr!("branch-tag-title"), tr!("branch-tag-button")),
    };
    ui.heading(title);
    ui.add_space(6.0);
    match &dialog.kind {
        NameKind::Create(start) => {
            ui.label(tr!(
                "branch-create-text",
                start = &start.label,
                id = start.target.short()
            ));
        }
        NameKind::Rename(old) => {
            ui.label(tr!("branch-rename-text", name = old));
        }
        NameKind::Tag(start) => {
            ui.label(tr!(
                "branch-tag-text",
                start = &start.label,
                id = start.target.short()
            ));
        }
    }
    ui.add_space(6.0);
    let id = ui.make_persistent_id("branch-name");
    // Enter in the field sends the name: read before the field, which would otherwise take it.
    let enter = ui.memory(|m| m.has_focus(id))
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let mut name = dialog.name.clone();
    let edit = egui::TextEdit::singleline(&mut name)
        .id(id)
        .hint_text(match &dialog.kind {
            NameKind::Tag(_) => tr!("branch-tag-name-hint"),
            _ => tr!("branch-name-hint"),
        })
        .desired_width(f32::INFINITY)
        .interactive(dialog.running.is_none());
    let response = ui.add(edit);
    // The field takes the keyboard on open.
    if dialog.running.is_none() && ui.memory(|m| m.focused().is_none()) {
        response.request_focus();
    }
    if response.changed() {
        actions.push(Action::Branch(branch::Action::Name(name)));
    }
    if matches!(dialog.kind, NameKind::Tag(_)) {
        ui.add_space(6.0);
        let mut message = dialog.message.clone();
        let edit = egui::TextEdit::multiline(&mut message)
            .hint_text(tr!("branch-tag-message-hint"))
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .interactive(dialog.running.is_none());
        if ui.add(edit).changed() {
            actions.push(Action::Branch(branch::Action::TagMessage(message)));
        }
    }
    if let Some(remote) = dialog.trackable() {
        ui.add_space(6.0);
        let mut track = dialog.track;
        let check = egui::Checkbox::new(
            &mut track,
            tr!("branch-track-checkbox", upstream = tracking::short(remote)),
        );
        let response = ui
            .add_enabled(dialog.running.is_none(), check)
            .on_hover_text(tr!("branch-track-hint"));
        if response.changed() {
            actions.push(Action::Branch(branch::Action::Track(track)));
        }
    }
    if let Some(error) = &dialog.error {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap());
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let clicked = ui
            .add_enabled(dialog.can_submit(), egui::Button::new(button))
            .clicked();
        if (clicked || enter) && dialog.can_submit() {
            actions.push(Action::Branch(branch::Action::SubmitName));
        }
        if ui
            .add_enabled(
                dialog.running.is_none(),
                egui::Button::new(tr!("branch-close")),
            )
            .clicked()
        {
            actions.push(Action::Branch(branch::Action::CloseDialog));
        }
        if dialog.running.is_some() {
            ui.spinner();
        }
    });
}

pub(super) fn delete_dialog(
    ui: &mut Ui,
    delete: &branch::DeleteDialog,
    reason: &str,
    actions: &mut Vec<Action>,
) {
    ui.heading(tr!(
        "branch-delete-heading",
        name = &delete.name,
        id = delete.target.short()
    ));
    ui.add_space(6.0);
    ui.add(egui::Label::new(tr!("branch-delete-unmerged")).wrap());
    ui.add_space(6.0);
    ui.strong(tr!("branch-reason"));
    ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
    ui.add_space(6.0);
    ui.add(egui::Label::new(tr!("branch-delete-backup")).wrap());
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = egui::Button::new(
            RichText::new(tr!("branch-delete-button")).color(ui.visuals().strong_text_color()),
        )
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
        if ui.add(button).clicked() {
            actions.push(Action::Branch(branch::Action::ConfirmDeleteBranch));
        }
        if ui.button(tr!("branch-dont-delete")).clicked() {
            actions.push(Action::Branch(branch::Action::CloseDialog));
        }
    });
}

/// Confirmation of the deletion of a local tag: the remote is not touched.
pub(super) fn delete_tag_dialog(
    ui: &mut Ui,
    delete: &branch::DeleteTagDialog,
    reason: &str,
    actions: &mut Vec<Action>,
) {
    ui.heading(tr!("branch-tag-delete-heading", name = &delete.name));
    ui.add_space(6.0);
    ui.add(egui::Label::new(tr!("branch-tag-delete-text")).wrap());
    ui.add_space(6.0);
    ui.strong(tr!("branch-reason"));
    ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
    ui.add_space(6.0);
    ui.add(egui::Label::new(tr!("branch-tag-delete-backup")).wrap());
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = egui::Button::new(
            RichText::new(tr!("branch-tag-delete-button")).color(ui.visuals().strong_text_color()),
        )
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
        if ui.add_enabled(delete.expected.is_some(), button).clicked() {
            actions.push(Action::Branch(branch::Action::ConfirmDeleteTag));
        }
        if ui.button(tr!("branch-dont-delete")).clicked() {
            actions.push(Action::Branch(branch::Action::CloseDialog));
        }
    });
}

/// Maximum height of the file list of the refused checkout; beyond it scrolls.
pub(super) const REFUSED_LIST_HEIGHT: f32 = 160.0;

pub(super) fn refused_dialog(ui: &mut Ui, refused: &branch::Refused, actions: &mut Vec<Action>) {
    ui.heading(tr!("branch-refused-heading", name = &refused.name));
    ui.add_space(6.0);
    ui.add(egui::Label::new(tr!("branch-refused-text")).wrap());
    ui.add_space(6.0);
    let n = refused.files.len();
    ui.strong(tr!("branch-refused-files", n = n));
    let row = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::vertical()
        .id_salt("checkout-files")
        .max_height(REFUSED_LIST_HEIGHT)
        .auto_shrink([false, true])
        .show_rows(ui, row, n, |ui, range| {
            for path in &refused.files[range] {
                ui.monospace(path);
            }
        });
    ui.add_space(10.0);
    if ui.button(tr!("branch-close")).clicked() {
        actions.push(Action::Branch(branch::Action::CloseDialog));
    }
}

/// Maximum height of the remote branch list of the "Set upstream" window; beyond it scrolls.
pub(super) const UPSTREAM_LIST_HEIGHT: f32 = 220.0;

/// The "Set upstream" window: filter, remote branches (virtualized), "Set upstream".
pub(super) fn upstream_dialog(
    ui: &mut Ui,
    dialog: &tracking::UpstreamDialog,
    actions: &mut Vec<Action>,
) {
    ui.heading(tr!("branch-upstream-title", name = &dialog.branch));
    ui.add_space(6.0);
    let text = match &dialog.current {
        Some(current) => tr!(
            "branch-upstream-text-current",
            upstream = tracking::short(current)
        ),
        None => tr!("branch-upstream-text-none"),
    };
    ui.add(egui::Label::new(text).wrap());
    ui.add_space(6.0);
    let idle = dialog.running.is_none();
    let id = ui.make_persistent_id("upstream-filter");
    let enter = ui.memory(|m| m.has_focus(id))
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let mut filter = dialog.filter.clone();
    let response = ui.add(
        egui::TextEdit::singleline(&mut filter)
            .id(id)
            .hint_text(tr!("branch-upstream-filter-hint"))
            .desired_width(f32::INFINITY)
            .interactive(idle),
    );
    if idle && ui.memory(|m| m.focused().is_none()) {
        response.request_focus();
    }
    if response.changed() {
        actions.push(Action::Branch(branch::Action::UpstreamFilter(filter)));
    }
    ui.add_space(4.0);
    let visible = dialog.visible();
    if dialog.choices.is_empty() {
        ui.weak(tr!("branch-upstream-no-remote-branches"));
    } else if visible.is_empty() {
        ui.weak(tr!("branch-upstream-no-match"));
    }
    let row = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
    egui::ScrollArea::vertical()
        .id_salt("upstream-choices")
        .max_height(UPSTREAM_LIST_HEIGHT)
        .auto_shrink([false, true])
        .show_rows(ui, row, visible.len(), |ui, range| {
            for full in &visible[range] {
                let selected = dialog.selected.as_deref() == Some(*full);
                let mut label = tracking::short(full).to_owned();
                if dialog.current.as_deref() == Some(*full) {
                    label = tr!("branch-upstream-current-mark", upstream = label);
                }
                let response = ui.add_enabled(
                    idle,
                    egui::Button::selectable(selected, label)
                        .min_size(vec2(ui.available_width(), 0.0)),
                );
                if response.clicked() {
                    actions.push(Action::Branch(branch::Action::UpstreamPick(
                        (*full).to_owned(),
                    )));
                }
                if response.double_clicked() {
                    actions.push(Action::Branch(branch::Action::SubmitUpstream));
                }
            }
        });
    if let Some(error) = &dialog.error {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap());
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let clicked = ui
            .add_enabled(
                dialog.can_submit(),
                egui::Button::new(tr!("branch-upstream-button")),
            )
            .clicked();
        if (clicked || enter) && dialog.can_submit() {
            actions.push(Action::Branch(branch::Action::SubmitUpstream));
        }
        if ui
            .add_enabled(idle, egui::Button::new(tr!("branch-close")))
            .clicked()
        {
            actions.push(Action::Branch(branch::Action::CloseDialog));
        }
        if !idle {
            ui.spinner();
        }
    });
}
