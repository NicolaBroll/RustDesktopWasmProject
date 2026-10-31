use super::*;
use crate::stash;

/// The "Stash" section: the stashes (`stash.list`), newest first, with the menu to apply them,
/// pop and drop them; a click shows the files on the right and the base in the graph. Below,
/// "Stash changes…".
pub(super) fn stash_section(
    ui: &mut Ui,
    repo: &RepoView,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let list = repo.stash.list();
    let title = match list {
        Some(list) => tr!("stash-section-title", n = list.len()),
        None => tr!("stash-section-title-loading"),
    };
    let mut body_actions = Vec::new();
    let add = |ui: &mut Ui| stash_push_button(ui, repo, can_write, add_button_widget(), actions);
    section(ui, "stash-section", icons::STASH, &title, true, add, |ui| {
        // Under the header's title, as the refs.
        let margin = egui::Margin {
            left: 20,
            ..egui::Margin::ZERO
        };
        egui::Frame::new()
            .inner_margin(margin)
            .show(ui, |ui| stash_rows(ui, repo, can_write, &mut body_actions));
    });
    actions.extend(body_actions);
}

/// The stashes, or why there are none.
fn stash_rows(ui: &mut Ui, repo: &RepoView, can_write: bool, actions: &mut Vec<Action>) {
    let list = repo.stash.list();
    match list {
        Some([]) => {
            ui.weak(tr!("stash-none"));
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
                ui.weak(tr!("stash-loading"));
            });
        }
        None => {}
    }
    if let Some(error) = repo.stash.list_error() {
        ui.add(
            egui::Label::new(
                RichText::new(tr!("stash-list-error", error = error))
                    .color(ui.visuals().error_fg_color)
                    .small(),
            )
            .wrap(),
        );
    }
}

/// The "+" of the section, as a widget for [`stash_push_button`].
fn add_button_widget() -> egui::Button<'static> {
    egui::Button::new(icons::text(icons::PLUS, 13.0)).frame(false)
}

/// The button that opens the "Stash" dialog: enabled if there are changes and no write in progress.
pub(super) fn stash_push_button(
    ui: &mut Ui,
    repo: &RepoView,
    can_write: bool,
    button: impl egui::Widget,
    actions: &mut Vec<Action>,
) {
    let changes = repo.has_changes();
    let button = ui
        .add_enabled(can_write && changes, button)
        .on_hover_text(tr!("stash-push-help"))
        .on_disabled_hover_text(if changes {
            wait()
        } else {
            tr!("stash-no-changes")
        });
    if button.clicked() {
        actions.push(Action::Stash(stash::Action::Open));
    }
}

/// A row of the "Stash" section.
pub(super) struct StashRow<'a> {
    stash: &'a StashEntry,
    selected: bool,
    can_write: bool,
    restore_index: bool,
    /// Already applied by a pop that did not remove it: no Pop.
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
            actions.push(Action::Stash(stash::Action::Select(stash.id)));
        }
        response.context_menu(|ui| stash_menu(ui, row, actions));
    });
    ui.horizontal(|ui| {
        let mut origin = stash.origin();
        if stash.untracked {
            origin = tr!("stash-origin-untracked", origin = origin);
        }
        ui.add(egui::Label::new(RichText::new(origin).small().weak()).truncate())
            .on_hover_text(tooltip);
    });
    ui.add_space(2.0);
}

/// The menu of a stash: apply, pop, "also restore the index", drop.
pub(super) fn stash_menu(ui: &mut Ui, row: &StashRow<'_>, actions: &mut Vec<Action>) {
    let flags = StashFlags {
        can_write: row.can_write,
        restore_index: row.restore_index,
        pop_blocked: row.pop_blocked,
        menu: true,
    };
    stash_actions(ui, row.stash, flags, actions);
}

/// Apply, Pop, the "Also restore the index" checkbox and Drop, for the menu (`menu`: closes
/// the menu after the click) and for the stash panel.
/// How to draw the actions of a stash.
#[derive(Clone, Copy)]
pub(super) struct StashFlags {
    can_write: bool,
    restore_index: bool,
    /// Already applied by a pop that did not remove it from the list: Pop disabled.
    pop_blocked: bool,
    /// In the context menu: the click closes it.
    menu: bool,
}

pub(super) fn stash_actions(
    ui: &mut Ui,
    stash: &StashEntry,
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
        .add_enabled(can_write, egui::Button::new(tr!("stash-apply-button")))
        .on_hover_text(tr!("stash-apply-help", name = name))
        .on_disabled_hover_text(wait());
    if button.clicked() {
        actions.push(Action::Stash(stash::Action::Apply(stash.id, false)));
        if menu {
            ui.close();
        }
    }
    let button = ui
        .add_enabled(
            can_write && !pop_blocked,
            egui::Button::new(tr!("stash-pop-button")),
        )
        .on_hover_text(tr!("stash-pop-help", name = name))
        .on_disabled_hover_text(if pop_blocked {
            tr!("stash-pop-blocked")
        } else {
            wait()
        });
    if button.clicked() {
        actions.push(Action::Stash(stash::Action::Apply(stash.id, true)));
        if menu {
            ui.close();
        }
    }
    let mut restore = restore_index;
    let check = ui
        .add_enabled(
            can_write,
            egui::Checkbox::new(&mut restore, tr!("stash-restore-index")),
        )
        .on_hover_text(tr!("stash-restore-index-help"));
    if check.changed() {
        actions.push(Action::Stash(stash::Action::RestoreIndex(restore)));
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
            egui::Button::new(RichText::new(tr!("stash-drop-button")).color(color)),
        )
        .on_hover_text(tr!("stash-drop-help"))
        .on_disabled_hover_text(wait());
    if button.clicked() {
        actions.push(Action::Stash(stash::Action::Drop(stash.id)));
        if menu {
            ui.close();
        }
    }
}

/// The selected stash, in place of the commit details: name, message, base, the actions and
/// the tracked files changed relative to the base (`commit.show` on the stash id; a click
/// opens the diff).
pub(crate) fn stash_panel(
    ui: &mut Ui,
    repo: &RepoView,
    stash: &StashEntry,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    let can_write = !writing && !repo.modal_open();
    ui.add_space(4.0);
    ui.add(egui::Label::new(RichText::new(stash.title()).heading()).wrap());
    ui.add_space(6.0);
    egui::Grid::new("stash-details")
        .num_columns(2)
        .spacing(vec2(12.0, 4.0))
        .show(ui, |ui| {
            ui.weak(tr!("stash-field-stash"));
            ui.monospace(&stash.name);
            ui.end_row();
            ui.weak(tr!("stash-field-commit"));
            ui.horizontal_wrapped(|ui| {
                let id = stash.id.to_string();
                ui.add(egui::Label::new(RichText::new(&id).monospace()).selectable(true));
                if ui
                    .small_button(tr!("stash-copy"))
                    .on_hover_text(tr!("stash-copy-help"))
                    .clicked()
                {
                    ui.ctx().copy_text(id);
                }
            });
            ui.end_row();
            ui.weak(tr!("stash-field-created-on"));
            ui.horizontal_wrapped(|ui| {
                oid_link(ui, stash.base, actions);
                match &stash.branch {
                    Some(branch) => ui.weak(tr!("stash-branch", branch = branch)),
                    None => ui.weak(tr!("stash-detached")),
                };
            });
            ui.end_row();
            ui.weak(tr!("stash-field-date"));
            ui.label(tr!("stash-date-utc", date = stash.date()));
            ui.end_row();
            ui.weak(tr!("stash-field-message"));
            ui.add(egui::Label::new(&stash.message).wrap());
            ui.end_row();
        });
    if stash.untracked {
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                RichText::new(tr!("stash-untracked-note")).color(ui.visuals().warn_fg_color),
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
    ui.weak(tr!("stash-changed-files"));
    match repo.details.shown() {
        Shown::Nothing => {}
        Shown::Loading(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(tr!("stash-files-loading"));
            });
        }
        Shown::Failed(_, error) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button(tr!("stash-retry")).clicked() {
                actions.push(Action::RetryDetails);
            }
        }
        Shown::Loaded(details) => {
            changed_files(
                ui,
                DiffSource::Commit(details.commit.id),
                details.commit.id,
                &details.files,
                repo.diff.target(),
                actions,
            );
        }
    }
}

/// The open stash dialog: "Stash" (push) or the drop confirmation.
pub(crate) fn stash_dialog(
    ctx: &egui::Context,
    repo: &RepoView,
    dialog: &stash::Dialog,
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("stash-dialog")).show(ctx, |ui| {
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
        actions.push(Action::Stash(stash::Action::CloseDialog));
    }
}

pub(super) fn push_stash_dialog(
    ui: &mut Ui,
    repo: &RepoView,
    dialog: &PushDialog,
    actions: &mut Vec<Action>,
) {
    ui.heading(tr!("stash-dialog-title"));
    ui.add_space(6.0);
    let untracked = repo
        .stage
        .unstaged
        .iter()
        .filter(|item| item.change == Change::Untracked)
        .count();
    ui.add(egui::Label::new(tr!("stash-dialog-body")).wrap());
    ui.add_space(8.0);
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let mut fields: PushFields = dialog.fields.clone();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(tr!("stash-dialog-message"));
        let message = ui.add(
            egui::TextEdit::singleline(&mut fields.message)
                .id(ui.make_persistent_id("stash-message"))
                .hint_text(tr!("stash-dialog-message-hint"))
                .desired_width(320.0)
                .interactive(dialog.running.is_none()),
        );
        if dialog.running.is_none() && ui.memory(|m| m.focused().is_none()) {
            message.request_focus();
        }
        changed |= message.changed();
    });
    ui.add_space(4.0);
    let label = match untracked {
        0 => tr!("stash-include-untracked"),
        n => tr!("stash-include-untracked-count", n = n),
    };
    let check = ui
        .add_enabled(
            dialog.running.is_none(),
            egui::Checkbox::new(&mut fields.include_untracked, label),
        )
        .on_hover_text(tr!("stash-include-untracked-help"));
    changed |= check.changed();
    let check = ui
        .add_enabled(
            dialog.running.is_none(),
            egui::Checkbox::new(&mut fields.keep_index, tr!("stash-keep-index")),
        )
        .on_hover_text(tr!("stash-keep-index-help"));
    changed |= check.changed();
    if changed {
        actions.push(Action::Stash(stash::Action::Fields(fields)));
    }
    if let Some(error) = &dialog.error {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap());
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let clicked = ui
            .add_enabled(
                dialog.running.is_none(),
                egui::Button::new(tr!("stash-submit")),
            )
            .clicked();
        if (clicked || enter) && dialog.running.is_none() {
            actions.push(Action::Stash(stash::Action::Submit));
        }
        if ui
            .add_enabled(
                dialog.running.is_none(),
                egui::Button::new(tr!("stash-close")),
            )
            .clicked()
        {
            actions.push(Action::Stash(stash::Action::CloseDialog));
        }
        if dialog.running.is_some() {
            ui.spinner();
            ui.label(tr!("stash-sending"));
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
        RichText::new(tr!("stash-drop-title", name = &drop.stash.name))
            .color(ui.visuals().error_fg_color),
    );
    ui.add_space(6.0);
    ui.add(egui::Label::new(tr!("stash-drop-body", title = drop.stash.title())).wrap());
    ui.add_space(6.0);
    ui.strong(tr!("stash-drop-what-happens"));
    ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
    ui.add_space(6.0);
    ui.add(egui::Label::new(tr!("stash-drop-undo")).wrap());
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = egui::Button::new(
            RichText::new(tr!("stash-drop-confirm")).color(ui.visuals().strong_text_color()),
        )
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
        if ui.add(button).clicked() {
            actions.push(Action::Stash(stash::Action::ConfirmDrop));
        }
        if ui.button(tr!("stash-drop-keep")).clicked() {
            actions.push(Action::Stash(stash::Action::CloseDialog));
        }
    });
}

/// Apply or pop stopped on conflicts: the stash is partly applied and stays in the list; the
/// conflicted files are resolved and staged from the changes area.
pub(crate) fn stash_conflicts_window(
    ctx: &egui::Context,
    conflicts: &stash::Conflicts,
    actions: &mut Vec<Action>,
) {
    let mut open = true;
    let pop = conflicts.op == stash::Op::Pop;
    let name = &conflicts.stash;
    let title = if pop {
        tr!("stash-conflicts-title-pop", name = name)
    } else {
        tr!("stash-conflicts-title-apply", name = name)
    };
    egui::Window::new(title)
        .id(egui::Id::unique("stash-conflicts"))
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.add(
                egui::Label::new(if pop {
                    tr!("stash-conflicts-body-pop", name = name)
                } else {
                    tr!("stash-conflicts-body-apply", name = name)
                })
                .wrap(),
            );
            ui.add_space(4.0);
            ui.add(egui::Label::new(tr!("stash-conflicts-how")).wrap());
            ui.add_space(6.0);
            let n = conflicts.files.len();
            ui.strong(tr!("stash-conflicts-files", n = n));
            let row = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::vertical()
                .id_salt("stash-conflicts-files")
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
                    .button(tr!("stash-go-to-changes"))
                    .on_hover_text(tr!("stash-go-to-changes-help"))
                    .clicked()
                {
                    actions.push(Action::SelectWorktree);
                    actions.push(Action::Stash(stash::Action::DismissConflicts));
                }
                if ui.button(tr!("stash-close")).clicked() {
                    actions.push(Action::Stash(stash::Action::DismissConflicts));
                }
            });
        });
    if !open {
        actions.push(Action::Stash(stash::Action::DismissConflicts));
    }
}
