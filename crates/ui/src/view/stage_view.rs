use super::*;

/// Area di stage, al posto dei dettagli quando è selezionata la riga delle modifiche: le liste
/// "In stage" e "Non in stage", ciascuna virtualizzata, e sotto la casella del messaggio di
/// commit. `writing` è vero mentre una scrittura è in corso: le azioni che modificano il repo
/// sono disabilitate. `draft` è la bozza del messaggio, che i campi di testo cambiano.
pub(crate) fn stage_panel(
    ui: &mut Ui,
    repo: &RepoView,
    draft: &mut CommitDraft,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    egui::Panel::bottom("messaggio-commit")
        .resizable(false)
        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 6)))
        .show(ui, |ui| commit_box(ui, repo, draft, writing, actions));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.heading("Modifiche non committate");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            stash_push_button(ui, repo, !writing && !repo.modal_open(), "Stash…", actions);
        });
    });
    if writing {
        ui.horizontal(|ui| {
            ui.spinner();
            let label = repo.write_label.as_deref().unwrap_or("Scrittura");
            ui.weak(format!("{label} in corso…"));
        });
    }
    if let Some(error) = &repo.write_error {
        ui.horizontal_top(|ui| {
            if ui.small_button("x").on_hover_text("Nascondi").clicked() {
                actions.push(Action::DismissWriteError);
            }
            ui.add(
                egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap(),
            );
        });
    }
    ui.separator();
    let enabled = !writing && !repo.modal_open();
    let open = repo.diff.target();
    // Le due liste si dividono lo spazio; una lista corta lascia il resto all'altra.
    let section_header = 28.0;
    let spare = (ui.available_height() - 2.0 * section_header - 12.0).max(80.0);
    let staged_h = list_height(repo.stage.staged.len())
        .min((spare / 2.0).max(spare - list_height(repo.stage.unstaged.len())));
    let sides = [
        (Side::Staged, &repo.stage.staged, staged_h),
        (Side::Unstaged, &repo.stage.unstaged, f32::INFINITY),
    ];
    for (i, (side, items, height)) in sides.into_iter().enumerate() {
        if i > 0 {
            ui.separator();
        }
        let height = height.min(ui.available_height() - section_header).max(40.0);
        stage_section(ui, side, items, open, enabled, height, actions);
    }
}

/// Righe del corpo del messaggio.
pub(super) const BODY_ROWS: usize = 4;

/// Altezza massima dell'output di git dopo un commit fallito; oltre si scorre.
pub(super) const OUTPUT_HEIGHT: f32 = 140.0;

/// Casella del messaggio di commit: oggetto con il contatore, corpo, amend, pulsante "Commit"
/// (anche Ctrl+Invio) e l'errore dell'ultimo commit.
pub(super) fn commit_box(
    ui: &mut Ui,
    repo: &RepoView,
    draft: &mut CommitDraft,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    let sending = repo.commit.sending().is_some();
    let gate = Gate {
        draft,
        ..repo.commit_gate(writing)
    }
    .check();
    // Prima dei campi di testo, che altrimenti riceverebbero il tasto.
    let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Enter);
    if !repo.modal_open() && ui.input_mut(|i| i.consume_shortcut(&shortcut)) && gate.is_ok() {
        actions.push(Action::Commit);
    }

    ui.horizontal(|ui| {
        ui.strong("Messaggio di commit");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let len = draft.subject_len();
            let text = format!("{len}/{}", commit::SUBJECT_LIMIT);
            if draft.subject_too_long() {
                ui.label(RichText::new(text).small().color(ui.visuals().warn_fg_color))
                    .on_hover_text(format!(
                        "L'oggetto supera {} caratteri: nel log e in molti strumenti viene tagliato. Il resto può andare nel corpo.",
                        commit::SUBJECT_LIMIT
                    ));
            } else {
                ui.label(RichText::new(text).small().weak())
                    .on_hover_text("Caratteri dell'oggetto");
            }
        });
    });
    ui.add_enabled(
        !sending,
        egui::TextEdit::singleline(&mut draft.subject)
            .id_salt("oggetto-commit")
            .hint_text("Oggetto (una riga breve)")
            .desired_width(f32::INFINITY),
    );
    ui.add_enabled(
        !sending,
        egui::TextEdit::multiline(&mut draft.body)
            .id_salt("corpo-commit")
            .hint_text("Descrizione (facoltativa)")
            .desired_rows(BODY_ROWS)
            .desired_width(f32::INFINITY),
    );

    let mut amend = draft.amend();
    let toggle = ui
        .add_enabled(
            !sending && !writing,
            egui::Checkbox::new(&mut amend, "Amend (modifica l'ultimo commit)"),
        )
        .on_hover_text(
            "Sostituisce l'ultimo commit con uno nuovo: stesso autore e genitori, il messaggio scritto qui e le modifiche in stage aggiunte. Il commit sostituito resta salvato in un ref di backup.",
        );
    if toggle.changed() {
        actions.push(Action::SetAmend(amend));
    }
    if draft.amend() {
        let what = match repo.head_commit() {
            Some(c) => format!("l'ultimo commit {} «{}»", c.id.short(), c.summary),
            None => match repo.head.target() {
                Some(id) => format!("l'ultimo commit {}", id.short()),
                None => "l'ultimo commit".to_owned(),
            },
        };
        let mut text = format!("Cambierà {what}: verrà sostituito da un commit nuovo.");
        if draft.is_proposed() {
            text.push_str(" Il messaggio è quello attuale: modificarlo se serve.");
        }
        ui.add(
            egui::Label::new(
                RichText::new(text)
                    .small()
                    .color(ui.visuals().warn_fg_color),
            )
            .wrap(),
        );
    }

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let label = if draft.amend() {
            "Commit (amend)"
        } else {
            "Commit"
        };
        let button = ui.add_enabled(gate.is_ok(), egui::Button::new(label));
        let button = match gate {
            Ok(()) => button.on_hover_text("Crea il commit (Ctrl+Invio)"),
            Err(blocked) => button.on_disabled_hover_text(blocked.hint()),
        };
        if button.clicked() {
            actions.push(Action::Commit);
        }
        if sending {
            ui.spinner();
            let label = repo.write_label.as_deref().unwrap_or("Commit");
            ui.weak(format!("{label} in corso…"));
        } else {
            ui.weak("Ctrl+Invio");
        }
    });
    // Perché il pulsante è spento, senza dover passare il mouse (non durante il lavoro).
    if let Err(blocked) = gate
        && !matches!(blocked, commit::Blocked::Busy)
    {
        ui.add(egui::Label::new(RichText::new(blocked.hint()).small().weak()).wrap());
    }

    if let Some(error) = repo.commit.error() {
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            if ui.small_button("x").on_hover_text("Nascondi").clicked() {
                actions.push(Action::DismissCommitError);
            }
            ui.add(
                egui::Label::new(RichText::new(&error.message).color(ui.visuals().error_fg_color))
                    .wrap(),
            );
        });
        if let Some(output) = &error.output {
            egui::ScrollArea::vertical()
                .id_salt("output-commit")
                .max_height(OUTPUT_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    // Un `&str` rende il campo di sola lettura, ma selezionabile e copiabile.
                    ui.add(
                        egui::TextEdit::multiline(&mut output.as_str())
                            .id_salt("output-commit-testo")
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(1)
                            .desired_width(f32::INFINITY),
                    );
                });
            if ui
                .small_button("Copia l'output")
                .on_hover_text("Copia negli appunti tutto l'output di git")
                .clicked()
            {
                ui.ctx().copy_text(output.clone());
            }
        }
    }
}

/// Altezza che serve a una lista di `rows` file, almeno una riga.
pub(super) fn list_height(rows: usize) -> f32 {
    rows.max(1) as f32 * (FILE_ROW_HEIGHT + 2.0)
}

/// Titolo con il conteggio, pulsante per tutta la lista, poi i file.
pub(super) fn stage_section(
    ui: &mut Ui,
    side: Side,
    items: &[StageItem],
    open: Option<&DiffTarget>,
    enabled: bool,
    height: f32,
    actions: &mut Vec<Action>,
) {
    let (title, op, all_label, all_help) = match side {
        Side::Staged => (
            "In stage",
            StageOp::Unstage,
            "Unstage di tutto",
            "Toglie dallo stage tutti i file di questa lista; il worktree non cambia",
        ),
        Side::Unstaged => (
            "Non in stage",
            StageOp::Stage,
            "Stage di tutto",
            "Mette in stage tutti i file di questa lista",
        ),
    };
    ui.horizontal(|ui| {
        ui.strong(format!("{title} ({})", items.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let button = ui
                .add_enabled(
                    enabled && !items.is_empty(),
                    egui::Button::new(all_label).small(),
                )
                .on_hover_text(all_help);
            if button.clicked()
                && let Some(paths) = stage::all_paths(items)
            {
                actions.push(Action::Write(op, paths));
            }
            if side == Side::Unstaged {
                let paths = discard::discardable_paths(items);
                let button = ui
                    .add_enabled(
                        enabled && paths.is_some(),
                        egui::Button::new("Scarta tutto…").small(),
                    )
                    .on_hover_text(
                        "Scarta le modifiche fuori stage di tutti i file tracciati di questa \
                         lista (chiede conferma, si può annullare). I file non tracciati non si \
                         toccano",
                    );
                if button.clicked()
                    && let Some(paths) = paths
                {
                    actions.push(Action::Discard(paths));
                }
            }
        });
    });
    if items.is_empty() {
        let text = match side {
            Side::Staged => "Nessun file in stage.",
            Side::Unstaged => "Nessuna modifica fuori dallo stage.",
        };
        ui.weak(text);
        return;
    }
    let source = match side {
        Side::Staged => DiffSource::Index,
        Side::Unstaged => DiffSource::Worktree,
    };
    let font = FontId::proportional(13.0);
    let mono = FontId::monospace(12.0);
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    let button_label = match op {
        StageOp::Stage => "Stage",
        StageOp::Unstage => "Unstage",
    };
    egui::ScrollArea::vertical()
        .id_salt(("area-stage", side == Side::Staged))
        .auto_shrink([false, true])
        .max_height(height)
        .show_rows(ui, FILE_ROW_HEIGHT, items.len(), |ui, range| {
            for item in &items[range] {
                let (rect, response) = ui.allocate_exact_size(
                    vec2(ui.available_width(), FILE_ROW_HEIGHT),
                    Sense::click(),
                );
                let target = DiffTarget {
                    source,
                    path: item.path.clone(),
                };
                if open == Some(&target) {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                } else if response.hovered() {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let painter = ui.painter_at(rect);
                let badge = egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 11.0, rect.center().y),
                    vec2(18.0, 16.0),
                );
                painter.rect_filled(badge, 3.0, change_color(item.change));
                painter.text(
                    badge.center(),
                    Align2::CENTER_CENTER,
                    change_letter(item.change),
                    mono.clone(),
                    Color32::from_gray(20),
                );

                // A destra il pulsante della riga.
                let button_w = 62.0;
                let button_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.right() - button_w, rect.top()),
                    rect.right_bottom(),
                );
                let mut cell = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(button_rect)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                let button = cell
                    .add_enabled(enabled, egui::Button::new(button_label).small())
                    .on_hover_text(row_action_help(op, item));

                let left = badge.right() + 6.0;
                let right = button_rect.left() - 4.0;
                let job = stage_path_job(item, &font, text, weak, right - left);
                let galley = painter.layout_job(job);
                let top = rect.center().y - galley.size().y / 2.0;
                painter.galley(egui::pos2(left, top), galley, text);

                let response = response.on_hover_text(stage_tooltip(op, item));
                if (button.clicked() || response.double_clicked()) && enabled {
                    actions.push(Action::Write(op, vec![item.path.clone()]));
                } else if response.clicked() {
                    actions.push(Action::OpenDiff(target.clone()));
                }
                response.context_menu(|ui| {
                    file_menu(ui, op, item, &target, enabled, actions);
                });
            }
        });
}

/// Il menu contestuale di un file dell'area di stage: diff, stage/unstage e, fuori stage,
/// "Scarta modifiche…".
pub(super) fn file_menu(
    ui: &mut Ui,
    op: StageOp,
    item: &StageItem,
    target: &DiffTarget,
    enabled: bool,
    actions: &mut Vec<Action>,
) {
    if ui.button("Mostra il diff").clicked() {
        actions.push(Action::OpenDiff(target.clone()));
        ui.close();
    }
    let label = match op {
        StageOp::Stage if item.is_conflict() => "Segna come risolto…",
        StageOp::Stage => "Stage",
        StageOp::Unstage => "Unstage",
    };
    let button = ui
        .add_enabled(enabled, egui::Button::new(label))
        .on_hover_text(row_action_help(op, item));
    if button.clicked() {
        actions.push(Action::Write(op, vec![item.path.clone()]));
        ui.close();
    }
    if op == StageOp::Stage {
        ui.separator();
        let reason = discard::not_discardable_reason(item);
        let button = ui
            .add_enabled(
                enabled && reason.is_none(),
                egui::Button::new(RichText::new("Scarta modifiche…").color(
                    if reason.is_none() && enabled {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().weak_text_color()
                    },
                )),
            )
            .on_hover_text(
                "Riporta il file alla versione in stage (o dell'ultimo commit): chiede conferma, \
                 si può annullare",
            )
            .on_disabled_hover_text(reason.unwrap_or("Aspettare l'operazione in corso"));
        if button.clicked() {
            actions.push(Action::Discard(vec![item.path.clone()]));
            ui.close();
        }
    }
}

pub(super) fn stage_path_job(
    item: &StageItem,
    font: &FontId,
    text: Color32,
    weak: Color32,
    width: f32,
) -> egui::text::LayoutJob {
    let format = |color| egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    if let Some(original) = &item.original {
        job.append(&format!("{original} -> "), 0.0, format(weak));
    }
    job.append(&item.path.to_string(), 0.0, format(text));
    job.wrap.max_width = width.max(10.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    job
}

pub(super) fn conflict_name(conflict: Conflict) -> &'static str {
    match conflict {
        Conflict::BothDeleted => "eliminato da entrambi",
        Conflict::AddedByUs => "aggiunto da noi",
        Conflict::DeletedByThem => "eliminato da loro",
        Conflict::AddedByThem => "aggiunto da loro",
        Conflict::DeletedByUs => "eliminato da noi",
        Conflict::BothAdded => "aggiunto da entrambi",
        Conflict::BothModified => "modificato da entrambi",
    }
}

pub(super) fn row_action_help(op: StageOp, item: &StageItem) -> &'static str {
    match op {
        StageOp::Stage if item.is_conflict() => "Segna il conflitto come risolto (chiede conferma)",
        StageOp::Stage => "Metti in stage questo file",
        StageOp::Unstage => "Togli dallo stage questo file; il worktree non cambia",
    }
}

pub(super) fn stage_tooltip(op: StageOp, item: &StageItem) -> String {
    let mut lines = Vec::new();
    let path = match &item.original {
        Some(original) => format!("{original} -> {}", item.path),
        None => item.path.to_string(),
    };
    match item.conflict {
        Some(conflict) => lines.push(format!(
            "{path} (in conflitto: {})",
            conflict_name(conflict)
        )),
        None => lines.push(format!("{path} ({})", format::change_name(item.change))),
    }
    lines.push("Clic: mostra il diff".to_owned());
    lines.push(
        match op {
            StageOp::Stage if item.is_conflict() => {
                "Doppio clic: segna il conflitto come risolto (chiede conferma)"
            }
            StageOp::Stage => "Doppio clic: metti in stage",
            StageOp::Unstage => "Doppio clic: togli dallo stage",
        }
        .to_owned(),
    );
    if op == StageOp::Stage && discard::discardable(item) {
        lines.push("Clic destro: altre azioni, anche scartare le modifiche".to_owned());
    } else {
        lines.push("Clic destro: altre azioni".to_owned());
    }
    lines.join("\n")
}
