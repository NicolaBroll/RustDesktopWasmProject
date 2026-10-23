use super::*;

/// La finestra di un'operazione non riuscita: cosa è successo, cosa fare, il testo di git. Non
/// blocca il resto della finestra; `dismiss` la chiude.
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
            if ui.button("Chiudi").clicked() {
                actions.push(dismiss.clone());
            }
        });
    if !open {
        actions.push(dismiss);
    }
}

/// La finestra di pull e push: errore, conflitto, rifiuto o conferma del force. `writing`:
/// c'è una scrittura in corso; `can_write`: nessuna scrittura né dialogo, i pulsanti che
/// riprovano sono attivi.
pub(crate) fn sync_window(
    ctx: &egui::Context,
    window: &Window,
    writing: bool,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    match window {
        Window::Failure(failure) => {
            failure_window(ctx, "errore-sync", failure, Action::DismissSync, actions);
        }
        Window::Conflict(conflict) => conflict_window(ctx, conflict, can_write, actions),
        Window::Rejected(rejected) => rejected_window(ctx, rejected, can_write, actions),
        Window::ConfirmForce(confirm) => confirm_force(ctx, confirm, writing, actions),
    }
}

/// Altezza massima dell'elenco dei file in conflitto; oltre si scorre.
pub(super) const CONFLICT_LIST_HEIGHT: f32 = 160.0;

/// Pull fermo in conflitto e già annullato dal motore (oggi solo il rebase: il merge fermo resta
/// in corso e si risolve dal riquadro dell'operazione).
pub(super) fn conflict_window(
    ctx: &egui::Context,
    conflict: &sync::Conflict,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let mut open = true;
    let operation = conflict.operation.to_string();
    egui::Window::new("Pull annullato: conflitti")
        .id(egui::Id::unique("conflitto-pull"))
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.add(
                egui::Label::new(format!(
                    "Il {operation} con l'upstream si è fermato in conflitto ed è stato \
                     annullato: il repo è com'era prima del pull (branch, indice e file non sono \
                     cambiati). Il fetch invece è fatto: il branch remoto mostra i commit \
                     arrivati."
                ))
                .wrap(),
            );
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(format!(
                    "Un rebase fermo non si continua ancora dall'app. Si può fare il pull con \
                     {}: se va in conflitto il merge resta in corso e i file si risolvono qui \
                     (poi «Continua il merge…» o «Annulla il merge…»); oppure continuare a \
                     lavorare sul branch e integrare dopo, o risolvere da terminale.",
                    conflict.other().name()
                ))
                .wrap(),
            );
            ui.add_space(6.0);
            let n = conflict.files.len();
            ui.strong(match n {
                0 => "Git non ha indicato i file.".to_owned(),
                1 => "File in conflitto:".to_owned(),
                n => format!("{n} file in conflitto:"),
            });
            let row = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::vertical()
                .id_salt("conflitto-file")
                .max_height(CONFLICT_LIST_HEIGHT)
                .auto_shrink([false, true])
                .show_rows(ui, row, n, |ui, range| {
                    for path in &conflict.files[range] {
                        ui.monospace(path);
                    }
                });
            ui.add_space(6.0);
            ui.collapsing("Messaggio del motore", |ui| {
                ui.add(egui::Label::new(RichText::new(&conflict.message).monospace()).wrap());
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let other = conflict.other();
                let button = ui
                    .add_enabled(
                        can_write,
                        egui::Button::new(format!("Pull con {}", other.name())),
                    )
                    .on_hover_text(format!(
                        "Riprova il pull con {} invece di {operation}, solo per questa volta \
                         (la configurazione non cambia)",
                        other.name()
                    ))
                    .on_disabled_hover_text(WAIT);
                if button.clicked() {
                    actions.push(Action::Pull(conflict.branch.clone(), Some(other)));
                }
                if ui.button("Chiudi").clicked() {
                    actions.push(Action::DismissSync);
                }
            });
        });
    if !open {
        actions.push(Action::DismissSync);
    }
}

/// Push rifiutato: cosa è successo e cosa fare. Non fast-forward: "Pull" e, come seconda
/// scelta, "Forza (con lease)…"; lease scaduto: "Fetch".
pub(super) fn rejected_window(
    ctx: &egui::Context,
    rejected: &sync::Rejected,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let mut open = true;
    egui::Window::new("Push rifiutato")
        .id(egui::Id::unique("push-rifiutato"))
        .collapsible(false)
        .resizable(false)
        .default_width(440.0)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
        .open(&mut open)
        .show(ctx, |ui| {
            if let Some(branch) = &rejected.branch {
                ui.strong(format!("Branch: «{branch}»"));
                ui.add_space(4.0);
            }
            ui.add(egui::Label::new(&rejected.explanation).wrap());
            git_text(ui, rejected.git.as_deref());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if rejected.non_fast_forward() {
                    let pull = ui
                        .add_enabled(can_write, egui::Button::new("Pull"))
                        .on_hover_text(
                            "Scarica e integra i commit del remoto, poi si potrà ripetere il push",
                        )
                        .on_disabled_hover_text(WAIT);
                    if pull.clicked() {
                        actions.push(Action::Pull(rejected.branch.clone(), None));
                    }
                }
                if rejected.stale() {
                    let fetch = ui
                        .add_enabled(can_write, egui::Button::new("Fetch"))
                        .on_hover_text("Scarica i commit arrivati sul remoto per controllarli")
                        .on_disabled_hover_text(WAIT);
                    if fetch.clicked() {
                        actions.push(Action::DismissSync);
                        actions.push(Action::Fetch(None));
                    }
                }
                if ui.button("Chiudi").clicked() {
                    actions.push(Action::DismissSync);
                }
                if rejected.non_fast_forward() {
                    ui.separator();
                    let force = ui
                        .add_enabled(
                            can_write,
                            egui::Button::new(
                                RichText::new("Forza (con lease)…")
                                    .color(ui.visuals().error_fg_color),
                            )
                            .frame(false),
                        )
                        .on_hover_text(
                            "Sovrascrive il branch sul remoto con il tuo (force-with-lease: \
                             rifiutato se il remoto si è spostato dall'ultimo fetch). Se il \
                             remoto perderebbe commit chiede conferma e li elenca",
                        )
                        .on_disabled_hover_text(WAIT);
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

/// Conferma del push forzato: il remoto perderebbe commit che il branch non ha.
pub(super) fn confirm_force(
    ctx: &egui::Context,
    confirm: &sync::ConfirmForce,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("conferma-force")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.heading(
            RichText::new(match &confirm.branch {
                Some(b) => format!("Push forzato di «{b}»: sovrascrivere il branch sul remoto?"),
                None => "Sovrascrivere il branch sul remoto?".to_owned(),
            })
            .color(ui.visuals().error_fg_color),
        );
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                "Il push forzato sostituisce il branch sul remoto con il tuo: i commit qui sotto \
                 spariscono dal remoto, anche per chi li ha già scaricati o ci sta lavorando. Un \
                 push non si annulla da qui.",
            )
            .wrap(),
        );
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                "Prima del push il branch remoto com'era si salva in un backup \
                 (refs/project/backup/push-…), da cui si può ripubblicare a mano. Con il lease, \
                 se nel frattempo il remoto si è spostato il push è rifiutato.",
            )
            .wrap(),
        );
        if confirm.changed {
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Il branch remoto è cambiato da quando hai visto questa conferma: \
                         niente è stato pubblicato. Controlla di nuovo i commit qui sotto.",
                    )
                    .strong()
                    .color(ui.visuals().warn_fg_color),
                )
                .wrap(),
            );
        }
        ui.add_space(6.0);
        ui.strong("Cosa si perde sul remoto:");
        egui::ScrollArea::vertical()
            .id_salt("force-motivo")
            .max_height(160.0)
            .show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&confirm.reason).monospace()).wrap());
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let button = egui::Button::new(
                RichText::new("Forza il push").color(ui.visuals().strong_text_color()),
            )
            .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
            let response = ui
                .add_enabled(!writing && confirm.can_confirm(), button)
                .on_disabled_hover_text(if confirm.can_confirm() {
                    WAIT
                } else {
                    "Il branch remoto locale non c'è più: chiudere, fare Fetch e riprovare"
                });
            if response.clicked() {
                actions.push(Action::ConfirmForcePush);
            }
            if ui.button("Non forzare").clicked() {
                actions.push(Action::DismissSync);
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::DismissSync);
    }
}
