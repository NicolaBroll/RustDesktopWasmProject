use super::*;

/// La sezione "Remoti": i remoti configurati (`remote.list`), con fetch e rimozione per
/// ciascuno, e "Aggiungi remoto…".
pub(super) fn remotes_section(
    ui: &mut Ui,
    repo: &RepoView,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let list = repo.remotes.list();
    let title = match list {
        Some(list) => format!("Remoti ({})", list.len()),
        None => "Remoti".to_owned(),
    };
    egui::CollapsingHeader::new(title)
        .id_salt("sezione-remoti")
        .default_open(true)
        .show(ui, |ui| {
            match list {
                Some([]) => {
                    ui.weak("Nessun remoto configurato");
                }
                Some(list) => {
                    for remote in list {
                        ui.push_id(("remoto", &remote.name), |ui| {
                            remote_row(ui, remote, can_write, actions);
                        });
                    }
                }
                None if repo.remotes.list_error().is_none() => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak("Lettura dei remoti…");
                    });
                }
                None => {}
            }
            if let Some(error) = repo.remotes.list_error() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("Remoti non letti: {error}"))
                            .color(ui.visuals().error_fg_color)
                            .small(),
                    )
                    .wrap(),
                );
            }
            let button = ui
                .add_enabled(can_write, egui::Button::new("Aggiungi remoto…"))
                .on_hover_text(
                    "Aggiunge un remoto (nome e URL) e, se scelto, ne scarica subito i branch. \
                     «Annulla» lo toglie",
                )
                .on_disabled_hover_text(WAIT);
            if button.clicked() {
                actions.push(Action::AddRemote);
            }
        });
}

/// Una riga della sezione "Remoti": nome, etichette, quanti branch, URL (mascherato dal
/// motore) e il menu con fetch e rimozione.
pub(super) fn remote_row(ui: &mut Ui, remote: &Remote, can_write: bool, actions: &mut Vec<Action>) {
    let tooltip = remote.tooltip();
    ui.horizontal(|ui| {
        let name = ui
            .add(egui::Label::new(RichText::new(&remote.name).strong()).sense(Sense::click()))
            .on_hover_text(format!("{tooltip}\nClic destro: fetch, rimozione"));
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
                .on_hover_text("Fetch, rimozione");
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

/// Il menu di un remoto: fetch e rimozione.
pub(super) fn remote_menu(
    ui: &mut Ui,
    remote: &Remote,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let name = &remote.name;
    let button = ui
        .add_enabled(can_write, egui::Button::new("Fetch"))
        .on_hover_text(format!(
            "Scarica commit, branch remoti e tag da «{name}». Non tocca branch locali, indice né \
             file"
        ))
        .on_disabled_hover_text(WAIT);
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
            egui::Button::new(RichText::new("Rimuovi…").color(color)),
        )
        .on_hover_text(
            "Toglie il remoto da questo repo, con i suoi branch remoti scaricati e gli upstream \
             che lo usano; prima chiede conferma. Il repository sul server non cambia; «Annulla» \
             lo rimette",
        )
        .on_disabled_hover_text(WAIT);
    if button.clicked() {
        actions.push(Action::RemoveRemote(name.clone()));
        ui.close();
    }
}

/// Il dialogo dei remoti aperto: aggiunta o conferma della rimozione. `progress`: l'ultimo
/// avanzamento del fetch dell'aggiunta in corso.
pub(crate) fn remote_dialog(
    ctx: &egui::Context,
    dialog: &remotes::Dialog,
    progress: Option<&Progress>,
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("dialogo-remoti")).show(ctx, |ui| {
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
    ui.heading("Aggiungi remoto");
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Un remoto è un altro repository (sul server o in una cartella) da cui scaricare e \
             su cui pubblicare. Si aggiunge con i branch remoti nome/* e niente cambia nei \
             branch locali; «Annulla» lo toglie.",
        )
        .wrap(),
    );
    ui.add_space(8.0);
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let mut fields: AddFields = dialog.fields.clone();
    let mut changed = false;
    egui::Grid::new("campi-remoto")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Nome");
            let name = ui.add(
                egui::TextEdit::singleline(&mut fields.name)
                    .id(ui.make_persistent_id("nome-remoto"))
                    .hint_text("es. origin, upstream")
                    .desired_width(320.0)
                    .interactive(!dialog.sending),
            );
            if !dialog.sending && ui.memory(|m| m.focused().is_none()) {
                name.request_focus();
            }
            changed |= name.changed();
            ui.end_row();
            ui.label("URL");
            if dialog.url_masked() {
                // Durante l'invio, o dopo "Nascondi": l'URL mascherato, in sola lettura.
                let mut shown = dialog.shown_url();
                ui.add(
                    egui::TextEdit::singleline(&mut shown)
                        .id(ui.make_persistent_id("url-remoto-mascherato"))
                        .desired_width(320.0)
                        .interactive(false),
                );
            } else {
                let url = ui.add(
                    egui::TextEdit::singleline(&mut fields.url)
                        .id(ui.make_persistent_id("url-remoto"))
                        .hint_text("https://host/progetto.git, git@host:utente/progetto.git")
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
                (
                    "Mostra l'URL",
                    "Mostra l'URL com'è scritto, per correggerlo",
                )
            } else {
                (
                    "Nascondi le credenziali",
                    "Mostra l'URL con le credenziali mascherate",
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
            egui::Checkbox::new(&mut fields.fetch, "Scarica subito (fetch)"),
        )
        .on_hover_text(
            "Dopo l'aggiunta scarica commit, branch remoti e tag. Se il fetch non riesce (o lo \
             si interrompe) il remoto resta aggiunto",
        );
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
            .add_enabled(dialog.can_submit(), egui::Button::new("Aggiungi"))
            .clicked();
        if (clicked || enter) && dialog.can_submit() {
            actions.push(Action::SubmitAddRemote);
        }
        if ui
            .add_enabled(!dialog.sending, egui::Button::new("Chiudi"))
            .clicked()
        {
            actions.push(Action::CloseRemoteDialog);
        }
        if dialog.sending {
            ui.spinner();
            if !dialog.fetching {
                ui.label("Aggiunta…");
            }
        }
    });
    if dialog.sending && dialog.fetching {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (text, fraction) = match progress {
                Some(p) => fetch::describe(p),
                None => ("Connessione al remoto…".to_owned(), None),
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
                .add_enabled(!dialog.cancelling, egui::Button::new("Interrompi"))
                .on_hover_text(
                    "Ferma il fetch: il remoto resta aggiunto, i branch remoti già scaricati \
                     restano",
                )
                .on_disabled_hover_text("Interruzione in corso…");
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
        RichText::new(format!("Rimuovere il remoto «{}»?", remove.name))
            .color(ui.visuals().error_fg_color),
    );
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Si tolgono da questo repo la configurazione del remoto e i suoi branch remoti \
             scaricati; i branch locali che lo seguono restano, ma senza upstream. Il \
             repository sul server non cambia e i tag scaricati restano.",
        )
        .wrap(),
    );
    ui.add_space(6.0);
    ui.strong("Cosa succede:");
    ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Con «Annulla» nella barra in alto il remoto torna com'era (URL, branch remoti, \
             upstream), finché l'app resta aperta: il diario delle operazioni è in memoria. \
             Dopo la chiusura restano solo i backup dei commit in refs/project/backup/remote-…: \
             il remoto si riaggiunge a mano e un Fetch riscarica i branch.",
        )
        .wrap(),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = egui::Button::new(
            RichText::new("Rimuovi il remoto").color(ui.visuals().strong_text_color()),
        )
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
        if ui.add(button).clicked() {
            actions.push(Action::ConfirmRemoveRemote);
        }
        if ui.button("Non rimuovere").clicked() {
            actions.push(Action::CloseRemoteDialog);
        }
    });
}
