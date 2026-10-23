use super::*;

/// Quanti file in conflitto elencare nella conferma; gli altri diventano "e altri N".
pub(super) const CONFIRM_LIST: usize = 8;

/// Conferma prima di mettere in stage file in conflitto: spiega che vengono segnati risolti.
pub(crate) fn confirm_write(ctx: &egui::Context, write: &PendingWrite, actions: &mut Vec<Action>) {
    let modal = egui::Modal::new(egui::Id::unique("conferma-scrittura")).show(ctx, |ui| {
        ui.set_max_width(440.0);
        let n = write.conflicts.len();
        ui.heading(if n == 1 {
            "Segnare il conflitto come risolto?"
        } else {
            "Segnare i conflitti come risolti?"
        });
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                "Mettere in stage un file in conflitto lo segna come risolto: nell'indice va il \
                 contenuto attuale del file nel worktree, così com'è, compresi eventuali \
                 marcatori di conflitto (<<<<<<<, =======, >>>>>>>) rimasti. Le versioni del \
                 conflitto (base, nostra e loro) escono dall'indice. Controllare prima che il \
                 file sia davvero risolto.",
            )
            .wrap(),
        );
        ui.add_space(6.0);
        ui.strong(if n == 1 {
            "File in conflitto:".to_owned()
        } else {
            format!("{n} file in conflitto:")
        });
        for path in write.conflicts.iter().take(CONFIRM_LIST) {
            ui.monospace(path.to_string_lossy());
        }
        if n > CONFIRM_LIST {
            ui.weak(format!("e altri {}", n - CONFIRM_LIST));
        }
        let others = write.paths.len() - n;
        if others > 0 {
            ui.add_space(4.0);
            ui.weak(format!(
                "Insieme a {others} altri file senza conflitti, messi in stage normalmente."
            ));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let label = if n == 1 {
                "Segna come risolto"
            } else {
                "Segna come risolti"
            };
            if ui.button(label).clicked() {
                actions.push(Action::ConfirmWrite);
            }
            if ui.button("Annulla").clicked() {
                actions.push(Action::CancelWrite);
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::CancelWrite);
    }
}

/// Altezza massima dell'elenco dei file nella conferma dello scarto; oltre si scorre.
pub(super) const DISCARD_LIST_HEIGHT: f32 = 160.0;

/// Conferma prima di scartare le modifiche fuori stage: elenca i file e spiega come si
/// annulla.
pub(crate) fn confirm_discard(ctx: &egui::Context, pending: &Discard, actions: &mut Vec<Action>) {
    let modal = egui::Modal::new(egui::Id::unique("conferma-scarto")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        let n = pending.paths.len();
        ui.heading("Scartare le modifiche?");
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                "Le modifiche non in stage vengono tolte dal worktree: ogni file torna come è \
                 nell'indice, cioè alla versione in stage, o a quella dell'ultimo commit se non \
                 ha niente in stage. Quello che è in stage resta com'è; i file non tracciati non \
                 vengono toccati.",
            )
            .wrap(),
        );
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                "Prima di scartare si salva un backup (refs/project/backup/discard-…): con \
                 «Annulla» nella barra in alto i file tornano come sono adesso, finché non \
                 vengono modificati di nuovo.",
            )
            .wrap(),
        );
        ui.add_space(6.0);
        ui.strong(if n == 1 {
            "File:".to_owned()
        } else {
            format!("{n} file:")
        });
        let row = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::vertical()
            .id_salt("scarto-file")
            .max_height(DISCARD_LIST_HEIGHT)
            .auto_shrink([false, true])
            .show_rows(ui, row, n, |ui, range| {
                for path in &pending.paths[range] {
                    ui.monospace(path.to_string_lossy());
                }
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let label = if n == 1 {
                "Scarta le modifiche"
            } else {
                "Scarta le modifiche di tutti"
            };
            let button =
                egui::Button::new(RichText::new(label).color(ui.visuals().strong_text_color()))
                    .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
            if ui.add(button).clicked() {
                actions.push(Action::ConfirmDiscard);
            }
            if ui.button("Non scartare").clicked() {
                actions.push(Action::CancelDiscard);
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::CancelDiscard);
    }
}

/// Conferma prima di riscrivere con amend un commit già su un branch remoto.
pub(crate) fn confirm_amend(
    ctx: &egui::Context,
    confirm: &ConfirmAmend,
    left_behind: &[String],
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("conferma-amend")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading("Riscrivere un commit già pubblicato?");
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                "L'ultimo commit è già pubblicato o etichettato: lo contiene un branch remoto o \
                 un tag. Con amend viene sostituito da un commit nuovo: i branch remoti e i tag \
                 restano sul commit vecchio, il branch locale diverge da quello remoto e per \
                 pubblicare la modifica servirà un push forzato, che riscrive la storia anche per \
                 chi ha già scaricato il commit. Il commit attuale resta salvato in un ref di \
                 backup.",
            )
            .wrap(),
        );
        ui.add_space(6.0);
        ui.strong("Motivo:");
        ui.add(egui::Label::new(RichText::new(&confirm.reason).monospace()).wrap());
        if !left_behind.is_empty() {
            ui.add_space(6.0);
            ui.strong("Restano sul commit vecchio anche:");
            for name in left_behind.iter().take(CONFIRM_LIST) {
                ui.monospace(name);
            }
            if left_behind.len() > CONFIRM_LIST {
                ui.weak(format!("e altri {}", left_behind.len() - CONFIRM_LIST));
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Riscrivi il commit").clicked() {
                actions.push(Action::ConfirmCommit);
            }
            if ui.button("Annulla").clicked() {
                actions.push(Action::CancelCommit);
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::CancelCommit);
    }
}
