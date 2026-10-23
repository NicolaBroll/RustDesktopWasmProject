use super::*;

/// La sezione "Stash": gli stash (`stash.list`), dal più recente, con il menu per applicarli,
/// fare pop ed eliminarli; un clic mostra i file a destra e la base nel grafo. Sotto,
/// "Stash delle modifiche…".
pub(super) fn stash_section(
    ui: &mut Ui,
    repo: &RepoView,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let list = repo.stash.list();
    let title = match list {
        Some(list) => format!("Stash ({})", list.len()),
        None => "Stash".to_owned(),
    };
    egui::CollapsingHeader::new(title)
        .id_salt("sezione-stash")
        .default_open(true)
        .show(ui, |ui| {
            match list {
                Some([]) => {
                    ui.weak("Nessuno stash");
                }
                Some(list) => {
                    let selected = repo.stash.selected().map(|s| s.id);
                    let restore_index = repo.stash.restore_index;
                    for stash in list {
                        ui.push_id(("stash", stash.id), |ui| {
                            let row = StashRow {
                                stash,
                                selected: selected == Some(stash.id),
                                can_write,
                                restore_index,
                                pop_blocked: repo.stash.pop_blocked(stash.id),
                            };
                            stash_row(ui, &row, actions);
                        });
                    }
                }
                None if repo.stash.list_error().is_none() => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak("Lettura degli stash…");
                    });
                }
                None => {}
            }
            if let Some(error) = repo.stash.list_error() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("Stash non letti: {error}"))
                            .color(ui.visuals().error_fg_color)
                            .small(),
                    )
                    .wrap(),
                );
            }
            stash_push_button(ui, repo, can_write, "Nuovo stash…", actions);
        });
}

/// Il pulsante che apre il dialogo "Stash": attivo se ci sono modifiche e nessuna scrittura.
pub(super) fn stash_push_button(
    ui: &mut Ui,
    repo: &RepoView,
    can_write: bool,
    label: &str,
    actions: &mut Vec<Action>,
) {
    let changes = repo.has_changes();
    let button = ui
        .add_enabled(can_write && changes, egui::Button::new(label))
        .on_hover_text(
            "Mette da parte le modifiche (in stage e no; a scelta anche i file non tracciati) e riporta i file come l'ultimo commit. «Annulla» le rimette",
        )
        .on_disabled_hover_text(if changes {
            WAIT
        } else {
            "Nessuna modifica da mettere da parte"
        });
    if button.clicked() {
        actions.push(Action::OpenStash);
    }
}

/// Una riga della sezione "Stash".
pub(super) struct StashRow<'a> {
    stash: &'a Stash,
    selected: bool,
    can_write: bool,
    restore_index: bool,
    /// Già applicato da un pop che non l'ha tolto: niente Pop.
    pop_blocked: bool,
}

pub(super) fn stash_row(ui: &mut Ui, row: &StashRow<'_>, actions: &mut Vec<Action>) {
    let stash = row.stash;
    let tooltip = stash.tooltip();
    ui.horizontal(|ui| {
        let text = RichText::new(format!("{} {}", stash.name, stash.title()));
        let response = ui
            .add(egui::Button::selectable(row.selected, text).truncate())
            .on_hover_text(&tooltip);
        if response.clicked() {
            actions.push(Action::SelectStash(stash.id));
        }
        response.context_menu(|ui| stash_menu(ui, row, actions));
    });
    ui.horizontal(|ui| {
        let mut origin = stash.origin();
        if stash.untracked {
            origin.push_str(" · con non tracciati");
        }
        ui.add(egui::Label::new(RichText::new(origin).small().weak()).truncate())
            .on_hover_text(tooltip);
    });
    ui.add_space(2.0);
}

/// Il menu di uno stash: applica, pop, "rimetti anche lo stage", elimina.
pub(super) fn stash_menu(ui: &mut Ui, row: &StashRow<'_>, actions: &mut Vec<Action>) {
    let flags = StashFlags {
        can_write: row.can_write,
        restore_index: row.restore_index,
        pop_blocked: row.pop_blocked,
        menu: true,
    };
    stash_actions(ui, row.stash, flags, actions);
}

/// Applica, Pop, la casella "Rimetti anche lo stage" ed Elimina, per il menu (`menu`: chiude
/// il menu dopo il clic) e per il pannello dello stash.
/// Come disegnare le azioni di uno stash.
#[derive(Clone, Copy)]
pub(super) struct StashFlags {
    can_write: bool,
    restore_index: bool,
    /// Già applicato da un pop che non l'ha tolto dall'elenco: Pop spento.
    pop_blocked: bool,
    /// Nel menu contestuale: il clic lo chiude.
    menu: bool,
}

pub(super) fn stash_actions(
    ui: &mut Ui,
    stash: &Stash,
    flags: StashFlags,
    actions: &mut Vec<Action>,
) {
    let StashFlags {
        can_write,
        restore_index,
        pop_blocked,
        menu,
    } = flags;
    let name = &stash.name;
    let button = ui
        .add_enabled(can_write, egui::Button::new("Applica"))
        .on_hover_text(format!(
            "Rimette nei file le modifiche di {name}; lo stash resta nell'elenco. Mai sopra file con modifiche locali: in quel caso niente cambia. Non si annulla con «Annulla»"
        ))
        .on_disabled_hover_text(WAIT);
    if button.clicked() {
        actions.push(Action::ApplyStash(stash.id, false));
        if menu {
            ui.close();
        }
    }
    let button = ui
        .add_enabled(can_write && !pop_blocked, egui::Button::new("Pop"))
        .on_hover_text(format!(
            "Applica {name} e, se non ci sono conflitti, lo toglie dall'elenco (il suo commit resta in un backup refs/project/backup/stash-…). Con conflitti lo stash resta. Non si annulla con «Annulla»"
        ))
        .on_disabled_hover_text(if pop_blocked {
            "Già applicato da un pop che non è riuscito a toglierlo dall'elenco: le modifiche \
             sono nei file, non va ripetuto. Per toglierlo, Elimina…"
        } else {
            WAIT
        });
    if button.clicked() {
        actions.push(Action::ApplyStash(stash.id, true));
        if menu {
            ui.close();
        }
    }
    let mut restore = restore_index;
    let check = ui
        .add_enabled(
            can_write,
            egui::Checkbox::new(&mut restore, "Rimetti anche lo stage"),
        )
        .on_hover_text(
            "Rimette in stage quello che era in stage quando lo stash è stato creato (--index); senza, tutto torna fuori stage tranne i file nuovi",
        );
    if check.changed() {
        actions.push(Action::StashRestoreIndex(restore));
    }
    if menu {
        ui.separator();
    }
    let color = if can_write {
        ui.visuals().error_fg_color
    } else {
        ui.visuals().weak_text_color()
    };
    let button = ui
        .add_enabled(
            can_write,
            egui::Button::new(RichText::new("Elimina…").color(color)),
        )
        .on_hover_text(
            "Toglie lo stash dall'elenco; prima chiede conferma. Il suo commit resta in un backup (refs/project/backup/stash-…) e «Annulla» lo rimette in cima all'elenco",
        )
        .on_disabled_hover_text(WAIT);
    if button.clicked() {
        actions.push(Action::DropStash(stash.id));
        if menu {
            ui.close();
        }
    }
}

/// Lo stash selezionato, al posto dei dettagli del commit: nome, messaggio, base, le azioni e
/// i file tracciati cambiati rispetto alla base (`commit.show` sull'id dello stash; un clic
/// apre il diff).
pub(crate) fn stash_panel(
    ui: &mut Ui,
    repo: &RepoView,
    stash: &Stash,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    let can_write = !writing && !repo.modal_open();
    ui.add_space(4.0);
    ui.add(egui::Label::new(RichText::new(stash.title()).heading()).wrap());
    ui.add_space(6.0);
    egui::Grid::new("dettagli-stash")
        .num_columns(2)
        .spacing(vec2(12.0, 4.0))
        .show(ui, |ui| {
            ui.weak("Stash");
            ui.monospace(&stash.name);
            ui.end_row();
            ui.weak("Commit");
            ui.horizontal_wrapped(|ui| {
                let id = stash.id.to_string();
                ui.add(egui::Label::new(RichText::new(&id).monospace()).selectable(true));
                if ui
                    .small_button("Copia")
                    .on_hover_text("Copia l'id completo")
                    .clicked()
                {
                    ui.ctx().copy_text(id);
                }
            });
            ui.end_row();
            ui.weak("Creato su");
            ui.horizontal_wrapped(|ui| {
                oid_link(ui, stash.base, actions);
                match &stash.branch {
                    Some(branch) => ui.weak(format!("(branch {branch})")),
                    None => ui.weak("(HEAD staccata)"),
                };
            });
            ui.end_row();
            ui.weak("Data");
            ui.label(format!("{} UTC", stash.date()));
            ui.end_row();
            ui.weak("Messaggio");
            ui.add(egui::Label::new(&stash.message).wrap());
            ui.end_row();
        });
    if stash.untracked {
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Contiene anche file non tracciati: non sono nell'elenco qui sotto, ma applicando lo stash tornano.",
                )
                .color(ui.visuals().warn_fg_color),
            )
            .wrap(),
        );
    }
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        let flags = StashFlags {
            can_write,
            restore_index: repo.stash.restore_index,
            pop_blocked: repo.stash.pop_blocked(stash.id),
            menu: false,
        };
        stash_actions(ui, stash, flags, actions);
    });
    ui.separator();
    ui.weak("File tracciati cambiati rispetto alla base:");
    match repo.details.shown() {
        Shown::Nothing => {}
        Shown::Loading(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Caricamento dei file dello stash…");
            });
        }
        Shown::Failed(_, error) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button("Riprova").clicked() {
                actions.push(Action::RetryDetails);
            }
        }
        Shown::Loaded(details) => {
            changed_files(
                ui,
                details.commit.id,
                &details.files,
                repo.diff.target(),
                actions,
            );
        }
    }
}

/// Il dialogo degli stash aperto: "Stash" (push) o la conferma dell'eliminazione.
pub(crate) fn stash_dialog(
    ctx: &egui::Context,
    repo: &RepoView,
    dialog: &stash::Dialog,
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("dialogo-stash")).show(ctx, |ui| {
        ui.set_max_width(480.0);
        match dialog {
            stash::Dialog::Push(push) => push_stash_dialog(ui, repo, push, actions),
            stash::Dialog::Drop(drop) => {
                let discard::Phase::Confirm { reason } = &drop.phase else {
                    return;
                };
                drop_stash_dialog(ui, drop, reason, actions);
            }
        }
    });
    if modal.should_close() {
        actions.push(Action::CloseStashDialog);
    }
}

pub(super) fn push_stash_dialog(
    ui: &mut Ui,
    repo: &RepoView,
    dialog: &PushDialog,
    actions: &mut Vec<Action>,
) {
    ui.heading("Stash delle modifiche");
    ui.add_space(6.0);
    let untracked = repo
        .stage
        .unstaged
        .iter()
        .filter(|item| item.change == Change::Untracked)
        .count();
    ui.add(
        egui::Label::new(
            "Le modifiche ai file tracciati (in stage e no) si mettono da parte in uno stash e i file tornano come l'ultimo commit. Si riprendono con Applica o Pop dalla sezione Stash; «Annulla» nella barra le rimette subito.",
        )
        .wrap(),
    );
    ui.add_space(8.0);
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let mut fields: PushFields = dialog.fields.clone();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Messaggio");
        let message = ui.add(
            egui::TextEdit::singleline(&mut fields.message)
                .id(ui.make_persistent_id("messaggio-stash"))
                .hint_text("facoltativo (es. a metà del parser)")
                .desired_width(320.0)
                .interactive(!dialog.sending),
        );
        if !dialog.sending && ui.memory(|m| m.focused().is_none()) {
            message.request_focus();
        }
        changed |= message.changed();
    });
    ui.add_space(4.0);
    let label = match untracked {
        0 => "Includi i file non tracciati".to_owned(),
        n => format!("Includi i file non tracciati ({n})"),
    };
    let check = ui
        .add_enabled(
            !dialog.sending,
            egui::Checkbox::new(&mut fields.include_untracked, label),
        )
        .on_hover_text(
            "Mette nello stash anche i file nuovi non ancora aggiunti (e li toglie dal disco); mai quelli ignorati. Senza, restano dove sono",
        );
    changed |= check.changed();
    let check = ui
        .add_enabled(
            !dialog.sending,
            egui::Checkbox::new(&mut fields.keep_index, "Tieni lo stage"),
        )
        .on_hover_text(
            "Quello che è in stage resta in stage (e nei file), ma è salvato anche nello stash; il resto si mette da parte",
        );
    changed |= check.changed();
    if changed {
        actions.push(Action::StashFields(fields));
    }
    if let Some(error) = &dialog.error {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap());
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let clicked = ui
            .add_enabled(!dialog.sending, egui::Button::new("Stash"))
            .clicked();
        if (clicked || enter) && !dialog.sending {
            actions.push(Action::SubmitStash);
        }
        if ui
            .add_enabled(!dialog.sending, egui::Button::new("Chiudi"))
            .clicked()
        {
            actions.push(Action::CloseStashDialog);
        }
        if dialog.sending {
            ui.spinner();
            ui.label("Stash…");
        }
    });
}

pub(super) fn drop_stash_dialog(
    ui: &mut Ui,
    drop: &DropDialog,
    reason: &str,
    actions: &mut Vec<Action>,
) {
    ui.heading(
        RichText::new(format!("Eliminare {}?", drop.stash.name)).color(ui.visuals().error_fg_color),
    );
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(format!(
            "Lo stash «{}» esce dall'elenco: le modifiche che contiene non si potranno più applicare da qui. I file del repo non cambiano.",
            drop.stash.title()
        ))
        .wrap(),
    );
    ui.add_space(6.0);
    ui.strong("Cosa succede:");
    ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Con «Annulla» nella barra in alto lo stash torna in cima all'elenco (stash@{0}), finché l'app resta aperta: il diario delle operazioni è in memoria. Dopo la chiusura resta il backup del suo commit in refs/project/backup/stash-….",
        )
        .wrap(),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = egui::Button::new(
            RichText::new("Elimina lo stash").color(ui.visuals().strong_text_color()),
        )
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
        if ui.add(button).clicked() {
            actions.push(Action::ConfirmDropStash);
        }
        if ui.button("Non eliminare").clicked() {
            actions.push(Action::CloseStashDialog);
        }
    });
}

/// Apply o pop fermo in conflitto: lo stash è applicato in parte e resta nell'elenco; i file
/// in conflitto si risolvono e si mettono in stage dall'area delle modifiche.
pub(crate) fn stash_conflicts_window(
    ctx: &egui::Context,
    conflicts: &stash::Conflicts,
    actions: &mut Vec<Action>,
) {
    let mut open = true;
    let what = match conflicts.op {
        stash::Op::Pop => "Pop",
        _ => "Applicazione",
    };
    egui::Window::new(format!("{what} di {}: conflitti", conflicts.stash))
        .id(egui::Id::unique("conflitti-stash"))
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.add(
                egui::Label::new(format!(
                    "Lo stash è stato applicato, ma dall'ultimo commit su cui era stato creato gli stessi file sono cambiati: quelli qui sotto sono in conflitto (con i marcatori di git dove git li ha messi). Gli altri file dello stash sono applicati. Niente si è perso: quei file non avevano modifiche locali e {} resta nell'elenco{}.",
                    conflicts.stash,
                    if conflicts.op == stash::Op::Pop {
                        " (il pop non lo elimina)"
                    } else {
                        ""
                    }
                ))
                .wrap(),
            );
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    "Per risolverli: correggere ogni file e metterlo in stage dall'area delle modifiche. Poi, se non serve più, eliminare lo stash. Finché restano conflitti né Stash né Applica sono possibili.",
                )
                .wrap(),
            );
            ui.add_space(6.0);
            let n = conflicts.files.len();
            ui.strong(match n {
                1 => "File in conflitto:".to_owned(),
                n => format!("{n} file in conflitto:"),
            });
            let row = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::vertical()
                .id_salt("conflitti-stash-file")
                .max_height(CONFLICT_LIST_HEIGHT)
                .auto_shrink([false, true])
                .show_rows(ui, row, n, |ui, range| {
                    for path in &conflicts.files[range] {
                        ui.monospace(path);
                    }
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .button("Vai alle modifiche")
                    .on_hover_text("Apre l'area di stage con i file in conflitto")
                    .clicked()
                {
                    actions.push(Action::SelectWorktree);
                    actions.push(Action::DismissStashConflicts);
                }
                if ui.button("Chiudi").clicked() {
                    actions.push(Action::DismissStashConflicts);
                }
            });
        });
    if !open {
        actions.push(Action::DismissStashConflicts);
    }
}
