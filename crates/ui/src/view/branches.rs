use super::*;

/// Il dialogo dei branch aperto: nome (nuovo o rinomina), conferma dell'eliminazione,
/// checkout rifiutato.
pub(crate) fn branch_dialog(ctx: &egui::Context, dialog: &Dialog, actions: &mut Vec<Action>) {
    let modal = egui::Modal::new(egui::Id::unique("dialogo-branch")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        match dialog {
            Dialog::Name(dialog) => name_dialog(ui, dialog, actions),
            Dialog::Delete(delete) => {
                let discard::Phase::Confirm { reason } = &delete.phase else {
                    return;
                };
                delete_dialog(ui, delete, reason, actions);
            }
            Dialog::Refused(refused) => refused_dialog(ui, refused, actions),
        }
    });
    if modal.should_close() {
        actions.push(Action::CloseBranchDialog);
    }
}

pub(super) fn name_dialog(ui: &mut Ui, dialog: &branch::NameDialog, actions: &mut Vec<Action>) {
    let (title, button) = match &dialog.kind {
        NameKind::Create(_) => ("Nuovo branch", "Crea il branch"),
        NameKind::Rename(_) => ("Rinomina il branch", "Rinomina"),
    };
    ui.heading(title);
    ui.add_space(6.0);
    match &dialog.kind {
        NameKind::Create(start) => {
            ui.label(format!(
                "Il branch parte da {} ({}). Si crea senza passarci: il branch attivo non cambia.",
                start.label,
                start.target.short()
            ));
        }
        NameKind::Rename(old) => {
            ui.label(format!(
                "Nuovo nome per «{old}». Reflog e upstream seguono il branch."
            ));
        }
    }
    ui.add_space(6.0);
    let id = ui.make_persistent_id("nome-branch");
    // Invio nel campo manda il nome: si legge prima del campo, che altrimenti lo prenderebbe.
    let enter = ui.memory(|m| m.has_focus(id))
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let mut name = dialog.name.clone();
    let edit = egui::TextEdit::singleline(&mut name)
        .id(id)
        .hint_text("es. feature/nuova")
        .desired_width(f32::INFINITY)
        .interactive(!dialog.sending);
    let response = ui.add(edit);
    // Il campo prende la tastiera all'apertura.
    if !dialog.sending && ui.memory(|m| m.focused().is_none()) {
        response.request_focus();
    }
    if response.changed() {
        actions.push(Action::BranchName(name));
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
            actions.push(Action::SubmitBranchName);
        }
        if ui
            .add_enabled(!dialog.sending, egui::Button::new("Chiudi"))
            .clicked()
        {
            actions.push(Action::CloseBranchDialog);
        }
        if dialog.sending {
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
    ui.heading(format!(
        "Eliminare «{}» ({})?",
        delete.name,
        delete.target.short()
    ));
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Il branch non è unito: ha commit che non sono nel branch attivo (né nel suo \
             upstream). Eliminandolo quei commit non sono più raggiungibili da nessun branch.",
        )
        .wrap(),
    );
    ui.add_space(6.0);
    ui.strong("Motivo:");
    ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Prima di eliminare si salva un backup del commit (refs/project/backup/delete-…): \
             con «Annulla» nella barra in alto il branch torna sullo stesso commit, con la \
             sua configurazione (upstream compreso), finché l'app resta aperta: il diario delle \
             operazioni è in memoria. Dopo la chiusura resta solo il backup del commit in \
             refs/project/backup/, da cui il branch si ricrea a mano.",
        )
        .wrap(),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = egui::Button::new(
            RichText::new("Elimina il branch").color(ui.visuals().strong_text_color()),
        )
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
        if ui.add(button).clicked() {
            actions.push(Action::ConfirmDeleteBranch);
        }
        if ui.button("Non eliminare").clicked() {
            actions.push(Action::CloseBranchDialog);
        }
    });
}

/// Altezza massima dell'elenco dei file del checkout rifiutato; oltre si scorre.
pub(super) const REFUSED_LIST_HEIGHT: f32 = 160.0;

pub(super) fn refused_dialog(ui: &mut Ui, refused: &branch::Refused, actions: &mut Vec<Action>) {
    ui.heading(format!("Checkout di «{}» non possibile", refused.name));
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Il checkout sovrascriverebbe modifiche locali: questi percorsi sono modificati (in \
             stage o no) o non tracciati (anche dentro una cartella che nel branch è un file, o \
             in un submodule), e nel branch di destinazione sono diversi. Niente è stato \
             modificato. Committare, scartare o mettere da parte queste modifiche e \
             riprovare.",
        )
        .wrap(),
    );
    ui.add_space(6.0);
    let n = refused.files.len();
    ui.strong(match n {
        0 => "Git non ha indicato i file.".to_owned(),
        1 => "File:".to_owned(),
        n => format!("{n} file:"),
    });
    let row = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::vertical()
        .id_salt("checkout-file")
        .max_height(REFUSED_LIST_HEIGHT)
        .auto_shrink([false, true])
        .show_rows(ui, row, n, |ui, range| {
            for path in &refused.files[range] {
                ui.monospace(path);
            }
        });
    ui.add_space(10.0);
    if ui.button("Chiudi").clicked() {
        actions.push(Action::CloseBranchDialog);
    }
}
