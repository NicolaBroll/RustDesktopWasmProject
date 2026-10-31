//! The credential modal (S52): what git or ssh asks, verbatim, and the field for the answer.

use super::*;
use crate::credentials::{self, Prompt};
use project_engine::CredentialKind;

/// The id of the answer field: its egui state is dropped when the modal closes.
pub(crate) fn credential_field_id() -> egui::Id {
    egui::Id::unique("credential-answer")
}

/// The open question. The field edits `prompt.answer` in place (the secret never goes in an
/// [`Action`]); "OK", "Cancel" and "Show" are actions.
pub(crate) fn credential_dialog(
    ctx: &egui::Context,
    prompt: &mut Prompt,
    actions: &mut Vec<Action>,
) {
    let kind = prompt.request.kind;
    let target = prompt
        .request
        .url
        .clone()
        .or_else(|| prompt.request.remote.clone())
        .unwrap_or_else(|| tr!("credentials-the-remote"));
    let push = |actions: &mut Vec<Action>, a| actions.push(Action::Credentials(a));
    let modal = egui::Modal::new(egui::Id::unique("credential-dialog")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.heading(match kind {
            CredentialKind::Username => tr!("credentials-title-username", target = &target),
            CredentialKind::Password => tr!("credentials-title-password", target = &target),
            CredentialKind::Passphrase => tr!("credentials-title-passphrase"),
            CredentialKind::HostKey => tr!("credentials-title-host-key"),
            CredentialKind::Other => tr!("credentials-title-other"),
        });
        if let Some(remote) = &prompt.request.remote {
            ui.weak(tr!("credentials-remote", remote = remote));
        }
        ui.add_space(6.0);
        // git's or ssh's own text: for a host key it holds the fingerprint.
        ui.add(
            egui::Label::new(RichText::new(prompt.request.prompt.trim_end()).monospace())
                .wrap()
                .selectable(true),
        );
        ui.add_space(8.0);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if kind == CredentialKind::HostKey {
            ui.add(egui::Label::new(tr!("credentials-host-key-explanation")).wrap());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                // No default: Enter does not trust the key.
                if ui.button(tr!("credentials-trust")).clicked() {
                    push(actions, credentials::Action::Answer);
                }
                if ui.button(tr!("credentials-cancel")).clicked() {
                    push(actions, credentials::Action::Cancel);
                }
            });
            return;
        }
        let secret = kind.is_secret();
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut prompt.answer)
                    .id(credential_field_id())
                    .password(secret && !prompt.shown)
                    .desired_width(if secret { 340.0 } else { 420.0 }),
            );
            if prompt.focus {
                field.request_focus();
                prompt.focus = false;
            }
            if secret {
                let label = if prompt.shown {
                    tr!("credentials-hide")
                } else {
                    tr!("credentials-show")
                };
                if ui.button(label).clicked() {
                    push(actions, credentials::Action::ToggleShown);
                }
            }
        });
        ui.add_space(4.0);
        let hint = match kind {
            CredentialKind::Passphrase => tr!("credentials-hint-passphrase"),
            _ => tr!("credentials-hint-helper"),
        };
        ui.add(egui::Label::new(RichText::new(hint).weak().small()).wrap());
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button(tr!("credentials-ok")).clicked() || enter {
                push(actions, credentials::Action::Answer);
            }
            if ui.button(tr!("credentials-cancel")).clicked() {
                push(actions, credentials::Action::Cancel);
            }
        });
    });
    // Escape only: a click outside must not throw away what was typed.
    if !modal.backdrop_response.clicked() && modal.should_close() {
        push(actions, credentials::Action::Cancel);
    }
}
