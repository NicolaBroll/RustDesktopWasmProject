use super::*;

pub(super) fn signer(sig: &Signature) -> String {
    format!("{} <{}>", sig.name, sig.email)
}

/// Panel on the right: message and metadata of the selected commit, then the changed files.
///
/// `selected` is the commit of the list: while `commit.show` works its message
/// and metadata are shown right away, the files arrive later. `open` is the file of the open diff, highlighted.
pub(crate) fn details_panel(
    ui: &mut Ui,
    selected: Option<&Commit>,
    shown: Shown<'_>,
    open: Option<&DiffTarget>,
    actions: &mut Vec<Action>,
) {
    let (commit, compared_to) = match &shown {
        Shown::Nothing => {
            ui.centered_and_justified(|ui| ui.weak(tr!("details-no-commit")));
            return;
        }
        Shown::Loaded(details) => (Some(&details.commit), Some(details.compared_to)),
        Shown::Loading(id) | Shown::Failed(id, _) => (selected.filter(|c| c.id == *id), None),
    };
    // The message can be long: it scrolls on its own and leaves at least half the panel to the files.
    let max_header = (ui.available_height() * 0.5).max(120.0);
    egui::ScrollArea::vertical()
        .id_salt("details-head")
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
                ui.weak(tr!("details-loading-files"));
            });
        }
        Shown::Failed(_, error) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button(tr!("details-retry")).clicked() {
                actions.push(Action::RetryDetails);
            }
        }
        Shown::Loaded(details) => {
            changed_files(ui, details.commit.id, &details.files, open, actions);
        }
    }
}

/// Subject, body and metadata. `compared_to` is `None` until `commit.show` answers.
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
    egui::Grid::new("details-commit")
        .num_columns(2)
        .spacing(vec2(12.0, 4.0))
        .show(ui, |ui| {
            ui.weak(tr!("details-commit"));
            // The short id on one line, the full one on hover and in the copy.
            ui.horizontal(|ui| {
                let id = commit.id.to_string();
                ui.add(egui::Label::new(
                    RichText::new(commit.id.short()).monospace(),
                ))
                .on_hover_text(&id);
                let copy = egui::Button::new(icons::text(icons::COPY, 13.0)).frame(false);
                if ui
                    .add(copy)
                    .on_hover_text(tr!("details-copy-hint"))
                    .clicked()
                {
                    ui.ctx().copy_text(id);
                }
            });
            ui.end_row();

            ui.weak(if commit.parents.len() == 1 {
                tr!("details-parent")
            } else {
                tr!("details-parents")
            });
            ui.horizontal_wrapped(|ui| {
                if commit.parents.is_empty() {
                    ui.weak(tr!("details-no-parents"));
                }
                for parent in &commit.parents {
                    oid_link(ui, *parent, actions);
                }
            });
            ui.end_row();

            for (title, sig) in [
                (tr!("details-author"), &commit.author),
                (tr!("details-committer"), &commit.committer),
            ] {
                ui.weak(title);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(signer(sig)).wrap());
                    ui.weak(date_time_zone(sig));
                });
                ui.end_row();
            }

            ui.weak(tr!("details-compared-to"));
            match compared_to {
                None => {
                    ui.weak("…");
                }
                Some(None) => {
                    ui.label(tr!("details-root-commit"));
                }
                Some(Some(base)) => {
                    ui.horizontal_wrapped(|ui| {
                        oid_link(ui, base, actions);
                        if commit.is_merge() {
                            ui.weak(tr!("details-first-parent"));
                        }
                    });
                }
            }
            ui.end_row();
        });
    if commit.is_merge() {
        ui.add_space(4.0);
        ui.colored_label(theme::MERGE, tr!("details-merge-commit"));
    }
    ui.add_space(4.0);
}

/// Height of a row of the file list.
pub(super) const FILE_ROW_HEIGHT: f32 = 20.0;

/// List of the changed files of commit `commit`, virtualized: only the visible rows
/// are drawn. A click on a file opens its diff; the file of the open diff (`open`) is highlighted.
pub(super) fn changed_files(
    ui: &mut Ui,
    commit: Oid,
    files: &[ChangedFile],
    open: Option<&DiffTarget>,
    actions: &mut Vec<Action>,
) {
    ui.strong(tr!("details-files-changed", n = files.len()));
    ui.add_space(2.0);
    let font = FontId::proportional(13.0);
    let mono = FontId::monospace(12.0);
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    egui::ScrollArea::vertical()
        .id_salt("details-file")
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
                let color = theme::change(file.change);
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
                    theme::ON_BADGE,
                );

                // On the right: similarity and submodule marker.
                let mut right = rect.right() - 4.0;
                let mut tail = file_score(file);
                if file.is_submodule() {
                    tail = tr!("details-submodule-tail", tail = tail)
                        .trim_start()
                        .to_owned();
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
                    actions.push(Action::OpenDiff(target.clone()));
                }
                response.context_menu(|ui| {
                    if ui.button(tr!("details-show-diff")).clicked() {
                        actions.push(Action::OpenDiff(target));
                        ui.close();
                    }
                    ui.separator();
                    let blame_off = (file.change == Change::Deleted)
                        .then(|| tr!("details-blame-deleted"))
                        .or_else(|| file.is_submodule().then(|| tr!("details-blame-submodule")));
                    file_menu_entries(ui, &file.path, Some(commit), blame_off, actions);
                });
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
        lines.push(tr!("details-similarity", score = score));
    }
    if file.old_mode != file.new_mode && file.old_mode != 0 && file.new_mode != 0 {
        lines.push(tr!(
            "details-mode",
            old = format!("{:o}", file.old_mode),
            new = format!("{:o}", file.new_mode)
        ));
    }
    if file.is_submodule() {
        lines.push(tr!("details-submodule"));
    }
    lines.push(tr!("details-click-diff"));
    lines.join("\n")
}
