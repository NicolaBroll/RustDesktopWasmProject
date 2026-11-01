//! Linked worktrees (S64): the "Worktrees" section of the refs column (only with more than one),
//! the "New worktree" dialog and the confirmation of "Remove…".

use super::*;
use crate::worktree::{self, AddDialog, AddFields, Mode, Remove, RemovePhase, WorktreeText};
use project_engine::WorktreeInfo;

/// Maximum height of the branch list in the dialog; beyond it, it scrolls.
const BRANCH_LIST_HEIGHT: f32 = 200.0;

/// "Worktrees": shown only when there is more than one (see [`WorktreeBox::shown`]); "+" in the
/// header opens the "New worktree" dialog.
///
/// [`WorktreeBox::shown`]: crate::worktree::WorktreeBox::shown
pub(crate) fn worktree_section(
    ui: &mut Ui,
    repo: &RepoView,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let list = repo.worktrees.list().unwrap_or_default();
    let title = tr!("worktree-section", n = list.len());
    let add = |ui: &mut Ui| {
        let button = add_button(ui, can_write)
            .on_hover_text(tr!("worktree-new-hint"))
            .on_disabled_hover_text(wait());
        if button.clicked() {
            actions.push(Action::Worktree(worktree::Action::OpenAdd(None)));
        }
    };
    let mut body_actions = Vec::new();
    section(
        ui,
        "worktree-section",
        icons::FOLDER,
        &title,
        true,
        add,
        |ui| {
            virtual_rows(ui, list.len(), |ui, i| {
                worktree_row(ui, &list[i], can_write, &mut body_actions);
            });
            if let Some(error) = repo.worktrees.list_error() {
                ui.add(
                    egui::Label::new(
                        RichText::new(tr!("worktree-list-error", error = error))
                            .color(ui.visuals().error_fg_color)
                            .small(),
                    )
                    .wrap(),
                );
            }
        },
    );
    actions.extend(body_actions);
}

/// A worktree: icon (a check on the one open here), folder name, branch or detached commit, the
/// badges and the "…" menu; right click opens the same menu, double click opens it in a tab.
fn worktree_row(ui: &mut Ui, info: &WorktreeInfo, can_write: bool, actions: &mut Vec<Action>) {
    let response = row_response(ui, ui.make_persistent_id(("worktree", &info.path)));
    ui.add_space(20.0);
    let icon = if info.current {
        icons::text(icons::CHECK, 13.0).color(ui.visuals().hyperlink_color)
    } else {
        icons::text(icons::FOLDER, 13.0).weak()
    };
    ui.label(icon);
    let name = RichText::new(info.short_path());
    let name = if info.current {
        name.strong()
    } else if info.prunable {
        name.weak().strikethrough()
    } else {
        name
    };
    // The menu and the badges first, on the right: the name takes the rest, cut if needed.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let more = egui::Button::new(icons::text(icons::MORE, 14.0)).frame(false);
        egui::containers::menu::MenuButton::from_button(more)
            .ui(ui, |ui| worktree_menu(ui, info, can_write, actions))
            .0
            .on_hover_text(tr!("worktree-menu-hint"));
        for (shown, text, color) in [
            (
                info.prunable,
                tr!("worktree-badge-prunable"),
                ui.visuals().warn_fg_color,
            ),
            (
                info.locked,
                tr!("worktree-badge-locked"),
                ui.visuals().warn_fg_color,
            ),
            (
                info.current,
                tr!("worktree-badge-current"),
                ui.visuals().weak_text_color(),
            ),
        ] {
            if shown {
                ui.label(RichText::new(text).small().color(color));
            }
        }
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(name).truncate().selectable(false));
            ui.add(
                egui::Label::new(RichText::new(info.what()).weak().small())
                    .truncate()
                    .selectable(false),
            );
        });
    });
    if response.double_clicked() && can_open(info).is_none() {
        actions.push(Action::Tabs(crate::tabs::Action::Open(info.path.clone())));
    }
    let response = response.on_hover_text(info.tooltip());
    response.context_menu(|ui| worktree_menu(ui, info, can_write, actions));
}

/// Why the worktree cannot be opened or shown, if it cannot.
fn can_open(info: &WorktreeInfo) -> Option<String> {
    if info.prunable {
        Some(tr!("worktree-tooltip-prunable"))
    } else if info.bare {
        Some(tr!("worktree-bare-hint"))
    } else {
        None
    }
}

/// The menu of a worktree.
fn worktree_menu(ui: &mut Ui, info: &WorktreeInfo, can_write: bool, actions: &mut Vec<Action>) {
    let off = can_open(info);
    let (label, hint) = if info.current {
        (
            tr!("worktree-open-current"),
            tr!("worktree-open-current-hint"),
        )
    } else {
        (tr!("worktree-open-tab"), tr!("worktree-open-tab-hint"))
    };
    if ui
        .add_enabled(off.is_none() && !info.current, egui::Button::new(label))
        .on_hover_text(hint.as_str())
        .on_disabled_hover_text(off.clone().unwrap_or(hint))
        .clicked()
    {
        actions.push(Action::Tabs(crate::tabs::Action::Open(info.path.clone())));
        ui.close();
    }
    if file_view::can_launch(ui)
        && ui
            .add_enabled(
                off.is_none(),
                egui::Button::new(tr!("worktree-show-folder")),
            )
            .on_hover_text(tr!("worktree-show-folder-hint"))
            .on_disabled_hover_text(off.unwrap_or_default())
            .clicked()
    {
        actions.push(Action::Tabs(crate::tabs::Action::ShowWorktreeFolder(
            info.path.clone(),
        )));
        ui.close();
    }
    ui.separator();
    let blocked = worktree::remove_blocked(info).or_else(|| (!can_write).then(wait));
    let color = if blocked.is_none() {
        ui.visuals().error_fg_color
    } else {
        ui.visuals().weak_text_color()
    };
    if ui
        .add_enabled(
            blocked.is_none(),
            egui::Button::new(RichText::new(tr!("worktree-remove")).color(color)),
        )
        .on_hover_text(tr!("worktree-remove-hint"))
        .on_disabled_hover_text(blocked.unwrap_or_default())
        .clicked()
    {
        actions.push(Action::Worktree(worktree::Action::Remove(
            info.path.clone(),
        )));
        ui.close();
    }
}

/// The "New worktree" dialog: folder ("Browse…" if the platform has a folder dialog), then what
/// to check out there: an existing branch, a new branch or a detached commit.
pub(crate) fn worktree_dialog(
    ctx: &egui::Context,
    dialog: &AddDialog,
    repo: &RepoView,
    can_pick: bool,
    actions: &mut Vec<Action>,
) {
    let running = dialog.running;
    let modal = egui::Modal::new(egui::Id::unique("worktree-dialog")).show(ctx, |ui| {
        ui.set_max_width(520.0);
        ui.heading(tr!("worktree-add-title"));
        ui.add_space(6.0);
        ui.add(egui::Label::new(tr!("worktree-add-explanation")).wrap());
        ui.add_space(8.0);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let mut fields = dialog.fields.clone();
        let mut changed = false;
        egui::Grid::new("worktree-fields")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label(tr!("worktree-add-path"));
                ui.horizontal(|ui| {
                    let width = if can_pick { 290.0 } else { 380.0 };
                    let path = ui.add(
                        egui::TextEdit::singleline(&mut fields.path)
                            .id(ui.make_persistent_id("worktree-path"))
                            .hint_text(tr!("worktree-add-path-hint"))
                            .desired_width(width)
                            .interactive(!running),
                    );
                    if !running && ui.memory(|m| m.focused().is_none()) {
                        path.request_focus();
                    }
                    changed |= path.changed();
                    if can_pick {
                        let browse = ui
                            .add_enabled(!running, egui::Button::new(tr!("clone-browse")))
                            .on_hover_text(tr!("worktree-add-browse-hint"));
                        if browse.clicked() {
                            actions.push(Action::Tabs(crate::tabs::Action::PickWorktreeFolder));
                        }
                    }
                });
                ui.end_row();
                ui.label(tr!("worktree-add-checkout"));
                ui.horizontal(|ui| {
                    for (mode, label, hint) in [
                        (
                            Mode::Existing,
                            tr!("worktree-mode-existing"),
                            tr!("worktree-mode-existing-hint"),
                        ),
                        (
                            Mode::New,
                            tr!("worktree-mode-new"),
                            tr!("worktree-mode-new-hint"),
                        ),
                        (
                            Mode::Detached,
                            tr!("worktree-mode-detached"),
                            tr!("worktree-mode-detached-hint"),
                        ),
                    ] {
                        let radio = ui
                            .add_enabled(
                                !running,
                                egui::RadioButton::new(fields.mode == mode, label),
                            )
                            .on_hover_text(hint);
                        if radio.clicked() && fields.mode != mode {
                            fields.mode = mode;
                            changed = true;
                        }
                    }
                });
                ui.end_row();
                changed |= mode_fields(ui, repo, &mut fields, running);
            });
        if changed {
            actions.push(Action::Worktree(worktree::Action::Fields(fields)));
        }
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(tr!("worktree-add-rule")).weak()).wrap());
        if let Some(error) = &dialog.error {
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap(),
            );
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let create = ui
                .add_enabled(
                    dialog.can_submit(),
                    egui::Button::new(tr!("worktree-add-submit")),
                )
                .on_disabled_hover_text(dialog.problem().unwrap_or_else(wait));
            if (create.clicked() || enter) && dialog.can_submit() {
                actions.push(Action::Worktree(worktree::Action::Submit));
            }
            if ui
                .add_enabled(!running, egui::Button::new(tr!("confirm-cancel")))
                .clicked()
            {
                actions.push(Action::Worktree(worktree::Action::CloseAdd));
            }
            if running {
                ui.spinner();
                ui.label(tr!("worktree-add-running"));
            }
        });
    });
    if !running && modal.should_close() {
        actions.push(Action::Worktree(worktree::Action::CloseAdd));
    }
}

/// The row of the dialog that depends on the mode: the branch to check out, or the name of the
/// new branch and where it starts, or the commit. True if a field changed.
fn mode_fields(ui: &mut Ui, repo: &RepoView, fields: &mut AddFields, running: bool) -> bool {
    let mut changed = false;
    match fields.mode {
        Mode::Existing => {
            ui.label(tr!("worktree-add-branch"));
            let current = view_current(repo);
            let free: Vec<&str> = repo
                .refs
                .iter()
                .filter(|r| r.kind == RefKind::LocalBranch)
                .map(RefInfo::short_name)
                .filter(|name| {
                    Some(*name) != current && repo.worktrees.branch_in_use(name).is_none()
                })
                .collect();
            let selected = if fields.branch.is_empty() {
                tr!("worktree-add-branch-pick")
            } else {
                fields.branch.clone()
            };
            ui.add_enabled_ui(!running, |ui| {
                egui::ComboBox::from_id_salt("worktree-branch")
                    .selected_text(selected)
                    .width(380.0)
                    .show_ui(ui, |ui| {
                        if free.is_empty() {
                            ui.weak(tr!("worktree-add-no-free-branch"));
                        }
                        let row = ui.spacing().interact_size.y;
                        egui::ScrollArea::vertical()
                            .id_salt("worktree-branches")
                            .max_height(BRANCH_LIST_HEIGHT)
                            .show_rows(ui, row, free.len(), |ui, range| {
                                for name in &free[range] {
                                    let on = fields.branch == *name;
                                    if ui.selectable_label(on, *name).clicked() && !on {
                                        fields.branch = (*name).to_owned();
                                        changed = true;
                                    }
                                }
                            });
                    })
                    .response
                    .on_hover_text(tr!("worktree-add-branch-hint"));
            });
            ui.end_row();
        }
        Mode::New => {
            ui.label(tr!("worktree-add-new-branch"));
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut fields.new_branch)
                        .id(ui.make_persistent_id("worktree-new-branch"))
                        .hint_text(tr!("worktree-add-new-branch-hint"))
                        .desired_width(380.0)
                        .interactive(!running),
                )
                .changed();
            ui.end_row();
            changed |= start_field(ui, fields, running);
        }
        Mode::Detached => changed |= start_field(ui, fields, running),
    }
    changed
}

/// The "Starting point" row: a branch, tag or commit; empty: HEAD.
fn start_field(ui: &mut Ui, fields: &mut AddFields, running: bool) -> bool {
    ui.label(tr!("worktree-add-start"));
    let changed = ui
        .add(
            egui::TextEdit::singleline(&mut fields.start)
                .id(ui.make_persistent_id("worktree-start"))
                .hint_text(tr!("worktree-add-start-hint"))
                .desired_width(380.0)
                .interactive(!running),
        )
        .changed();
    ui.end_row();
    changed
}

/// The short name of the current branch, if any.
fn view_current(repo: &RepoView) -> Option<&str> {
    match &repo.head {
        Head::Branch { name, .. } => Some(name.as_str()),
        Head::Detached(_) => None,
    }
}

/// "Remove…": the confirmation with the engine's reason (whether a backup is saved), or the
/// engine's refusal with only "Close". "Cancel" has the focus and Enter never removes: only a
/// click (or Space on the focused button) does.
pub(crate) fn confirm_worktree_remove(
    ctx: &egui::Context,
    remove: &Remove,
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("worktree-remove")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        match &remove.phase {
            RemovePhase::Confirm(reason) => {
                ui.heading(tr!("worktree-remove-title"));
                ui.add_space(6.0);
                ui.monospace(&remove.path);
                ui.add_space(6.0);
                ui.add(egui::Label::new(tr!("worktree-remove-body")).wrap());
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(reason).weak()).wrap());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let label = tr!("worktree-remove-button");
                    let button = egui::Button::new(
                        RichText::new(label).color(ui.visuals().strong_text_color()),
                    )
                    .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.add(button).clicked() && !enter {
                        actions.push(Action::Worktree(worktree::Action::ConfirmRemove));
                    }
                    let cancel = ui.button(tr!("confirm-cancel"));
                    if ui.memory(|m| m.focused().is_none()) {
                        cancel.request_focus();
                    }
                    if cancel.clicked() {
                        actions.push(Action::Worktree(worktree::Action::CancelRemove));
                    }
                });
            }
            RemovePhase::Refused(message) => {
                ui.heading(tr!("worktree-remove-refused-title"));
                ui.add_space(6.0);
                ui.monospace(&remove.path);
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(RichText::new(message).color(ui.visuals().error_fg_color))
                        .wrap(),
                );
                ui.add_space(10.0);
                let close = ui.button(tr!("worktree-close"));
                if ui.memory(|m| m.focused().is_none()) {
                    close.request_focus();
                }
                if close.clicked() {
                    actions.push(Action::Worktree(worktree::Action::CancelRemove));
                }
            }
            RemovePhase::Checking | RemovePhase::Sending => {}
        }
    });
    if modal.should_close() {
        actions.push(Action::Worktree(worktree::Action::CancelRemove));
    }
}
