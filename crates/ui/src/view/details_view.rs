use super::*;

pub(super) fn signer(sig: &Signature) -> String {
    format!("{} <{}>", sig.name, sig.email)
}

/// Pannello a destra: messaggio e metadati del commit selezionato, poi i file cambiati.
///
/// `selected` è il commit della lista: mentre `commit.show` lavora se ne mostrano subito
/// messaggio e metadati, i file arrivano dopo. `open` è il file del diff aperto, evidenziato.
pub(crate) fn details_panel(
    ui: &mut Ui,
    selected: Option<&Commit>,
    shown: Shown<'_>,
    open: Option<&DiffTarget>,
    actions: &mut Vec<Action>,
) {
    let (commit, compared_to) = match &shown {
        Shown::Nothing => {
            ui.centered_and_justified(|ui| ui.weak("Nessun commit selezionato."));
            return;
        }
        Shown::Loaded(details) => (Some(&details.commit), Some(details.compared_to)),
        Shown::Loading(id) | Shown::Failed(id, _) => (selected.filter(|c| c.id == *id), None),
    };
    // Il messaggio può essere lungo: scorre da solo e lascia ai file almeno metà pannello.
    let max_header = (ui.available_height() * 0.5).max(120.0);
    egui::ScrollArea::vertical()
        .id_salt("dettagli-testa")
        .max_height(max_header)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if let Some(commit) = commit {
                commit_header(ui, commit, compared_to, actions);
            }
        });
    ui.separator();
    match shown {
        Shown::Nothing => {}
        Shown::Loading(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Caricamento dei file cambiati…");
            });
        }
        Shown::Failed(_, error) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button("Riprova").clicked() {
                actions.push(Action::RetryDetails);
            }
        }
        Shown::Loaded(details) => {
            changed_files(ui, details.commit.id, &details.files, open, actions);
        }
    }
}

/// Oggetto, corpo e metadati. `compared_to` è `None` finché `commit.show` non risponde.
pub(super) fn commit_header(
    ui: &mut Ui,
    commit: &Commit,
    compared_to: Option<Option<Oid>>,
    actions: &mut Vec<Action>,
) {
    ui.add_space(4.0);
    ui.add(egui::Label::new(RichText::new(&commit.summary).heading()).wrap());
    if !commit.body.is_empty() {
        ui.add_space(4.0);
        ui.add(egui::Label::new(&commit.body).wrap());
    }
    ui.add_space(8.0);
    egui::Grid::new("dettagli-commit")
        .num_columns(2)
        .spacing(vec2(12.0, 4.0))
        .show(ui, |ui| {
            ui.weak("Commit");
            ui.horizontal_wrapped(|ui| {
                let id = commit.id.to_string();
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

            ui.weak(if commit.parents.len() == 1 {
                "Genitore"
            } else {
                "Genitori"
            });
            ui.horizontal_wrapped(|ui| {
                if commit.parents.is_empty() {
                    ui.weak("nessuno");
                }
                for parent in &commit.parents {
                    oid_link(ui, *parent, actions);
                }
            });
            ui.end_row();

            for (title, sig) in [("Autore", &commit.author), ("Committer", &commit.committer)] {
                ui.weak(title);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(signer(sig)).wrap());
                    ui.weak(date_time_zone(sig));
                });
                ui.end_row();
            }

            ui.weak("Confrontato con");
            match compared_to {
                None => {
                    ui.weak("…");
                }
                Some(None) => {
                    ui.label("commit radice (albero vuoto)");
                }
                Some(Some(base)) => {
                    ui.horizontal_wrapped(|ui| {
                        oid_link(ui, base, actions);
                        if commit.is_merge() {
                            ui.weak("(primo genitore)");
                        }
                    });
                }
            }
            ui.end_row();
        });
    if commit.is_merge() {
        ui.add_space(4.0);
        ui.colored_label(Color32::from_rgb(0x9b, 0x6b, 0xdf), "Commit di merge");
    }
    ui.add_space(4.0);
}

/// Altezza di una riga della lista dei file.
pub(super) const FILE_ROW_HEIGHT: f32 = 20.0;

/// Elenco dei file cambiati del commit `commit`, virtualizzato: si disegnano solo le righe
/// visibili. Un clic su un file ne apre il diff; il file del diff aperto (`open`) è evidenziato.
pub(super) fn changed_files(
    ui: &mut Ui,
    commit: Oid,
    files: &[ChangedFile],
    open: Option<&DiffTarget>,
    actions: &mut Vec<Action>,
) {
    ui.strong(match files.len() {
        0 => "Nessun file cambiato".to_owned(),
        1 => "1 file cambiato".to_owned(),
        n => format!("{n} file cambiati"),
    });
    ui.add_space(2.0);
    let font = FontId::proportional(13.0);
    let mono = FontId::monospace(12.0);
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    egui::ScrollArea::vertical()
        .id_salt("dettagli-file")
        .auto_shrink(false)
        .show_rows(ui, FILE_ROW_HEIGHT, files.len(), |ui, range| {
            for file in &files[range] {
                let (rect, response) = ui.allocate_exact_size(
                    vec2(ui.available_width(), FILE_ROW_HEIGHT),
                    Sense::click(),
                );
                let target = DiffTarget::commit(commit, file.path.clone());
                if open == Some(&target) {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                } else if response.hovered() {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let painter = ui.painter_at(rect);
                let color = change_color(file.change);
                let badge = egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 11.0, rect.center().y),
                    vec2(18.0, 16.0),
                );
                painter.rect_filled(badge, 3.0, color);
                painter.text(
                    badge.center(),
                    Align2::CENTER_CENTER,
                    change_letter(file.change),
                    mono.clone(),
                    Color32::from_gray(20),
                );

                // A destra: somiglianza e segno del sottomodulo.
                let mut right = rect.right() - 4.0;
                let mut tail = file_score(file);
                if file.is_submodule() {
                    tail = format!("{tail} [sottomodulo]").trim_start().to_owned();
                }
                if !tail.is_empty() {
                    let galley = painter.layout_no_wrap(tail, font.clone(), weak);
                    right -= galley.size().x;
                    let top = rect.center().y - galley.size().y / 2.0;
                    painter.galley(egui::pos2(right, top), galley, weak);
                    right -= 8.0;
                }

                let left = badge.right() + 6.0;
                let galley = painter.layout_job(path_job(file, &font, text, weak, right - left));
                let top = rect.center().y - galley.size().y / 2.0;
                painter.galley(egui::pos2(left, top), galley, text);

                let response = response.on_hover_text(file_tooltip(file));
                if response.clicked() {
                    actions.push(Action::OpenDiff(target));
                }
            }
        });
}

pub(super) fn file_tooltip(file: &ChangedFile) -> String {
    let mut lines = vec![format!(
        "{} ({})",
        file_path(file),
        format::change_name(file.change)
    )];
    if let Some(score) = file.score {
        lines.push(format!("somiglianza {score}%"));
    }
    if file.old_mode != file.new_mode && file.old_mode != 0 && file.new_mode != 0 {
        lines.push(format!("modo {:o} -> {:o}", file.old_mode, file.new_mode));
    }
    if file.is_submodule() {
        lines.push("sottomodulo".to_owned());
    }
    lines.push("Clic: mostra il diff".to_owned());
    lines.join("\n")
}
