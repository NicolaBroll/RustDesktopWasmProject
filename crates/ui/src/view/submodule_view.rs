//! The "Submodules" section of the refs column (S62): each submodule with its state, "Update all"
//! in the header and the menu of a row (update, open in a tab, show the folder).

use super::*;
use crate::submodule::{self, SubmoduleText};
use project_engine::{SubmoduleInfo, SubmoduleState};

/// "Submodules": shown only when the repo has any (see [`SubmoduleBox::shown`]).
///
/// [`SubmoduleBox::shown`]: crate::submodule::SubmoduleBox::shown
pub(crate) fn submodule_section(
    ui: &mut Ui,
    repo: &RepoView,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let list = repo.submodules.list().unwrap_or_default();
    let can_update = can_write && repo.submodules.running().is_none();
    let title = tr!("submodule-section", n = list.len());
    let all = |ui: &mut Ui| {
        let button = ui
            .add_enabled(
                can_update,
                egui::Button::new(icons::text(icons::REFRESH, 13.0)).frame(false),
            )
            .on_hover_text(tr!("submodule-update-all-hint"))
            .on_disabled_hover_text(wait());
        if button.clicked() {
            actions.push(Action::Submodule(submodule::Action::Update {
                path: None,
                init: true,
            }));
        }
    };
    let mut body_actions = Vec::new();
    section(
        ui,
        "submodule-section",
        icons::FOLDER,
        &title,
        true,
        all,
        |ui| {
            virtual_rows(ui, list.len(), |ui, i| {
                submodule_row(ui, &list[i], can_update, &mut body_actions);
            });
            if let Some(problems) = repo.submodules.problems() {
                ui.add(
                    egui::Label::new(
                        RichText::new(problems)
                            .color(ui.visuals().warn_fg_color)
                            .small(),
                    )
                    .wrap(),
                );
            }
            if let Some(error) = repo.submodules.list_error() {
                ui.add(
                    egui::Label::new(
                        RichText::new(tr!("submodule-list-error", error = error))
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

/// The color of the state badge.
fn badge_color(ui: &Ui, state: SubmoduleState) -> Color32 {
    match state {
        SubmoduleState::Conflict => ui.visuals().error_fg_color,
        SubmoduleState::LocalChanges | SubmoduleState::DifferentCommit => {
            ui.visuals().warn_fg_color
        }
        SubmoduleState::Uninitialized | SubmoduleState::InSync => ui.visuals().weak_text_color(),
    }
}

/// A submodule: icon, path, state badge and the "…" menu; right click opens the same menu.
fn submodule_row(ui: &mut Ui, info: &SubmoduleInfo, can_update: bool, actions: &mut Vec<Action>) {
    let response = row_response(ui, ui.make_persistent_id(("submodule", &info.path)));
    ui.add_space(20.0);
    ui.label(icons::text(icons::FOLDER, 13.0).weak());
    let path = RichText::new(info.path_text());
    let path = if info.needs_init() { path.weak() } else { path };
    // The menu and the badge first, on the right: the path takes the rest, cut if needed.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let more = egui::Button::new(icons::text(icons::MORE, 14.0)).frame(false);
        egui::containers::menu::MenuButton::from_button(more)
            .ui(ui, |ui| submodule_menu(ui, info, can_update, actions))
            .0
            .on_hover_text(tr!("submodule-menu-hint"));
        ui.label(
            RichText::new(info.badge())
                .small()
                .color(badge_color(ui, info.state)),
        );
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(path).truncate().selectable(false));
        });
    });
    let response = response.on_hover_text(info.tooltip());
    response.context_menu(|ui| submodule_menu(ui, info, can_update, actions));
}

/// Why "Update" is off for this submodule, if it is: nothing to do, or the engine would refuse it.
fn update_blocked(info: &SubmoduleInfo) -> Option<String> {
    match info.state {
        SubmoduleState::InSync => Some(tr!("submodule-explain-in-sync")),
        SubmoduleState::Conflict => Some(tr!("submodule-explain-conflict")),
        SubmoduleState::LocalChanges => Some(tr!("submodule-explain-local-changes")),
        SubmoduleState::Uninitialized | SubmoduleState::DifferentCommit => {
            if info.operation.is_some() {
                Some(tr!("submodule-blocked-operation"))
            } else if info.unreferenced {
                Some(tr!("submodule-tooltip-unreferenced"))
            } else if info.ignored_overwritten {
                Some(tr!("submodule-tooltip-ignored"))
            } else {
                None
            }
        }
    }
}

/// The menu of a submodule.
fn submodule_menu(ui: &mut Ui, info: &SubmoduleInfo, can_update: bool, actions: &mut Vec<Action>) {
    let init = info.needs_init();
    let (label, hint) = if init {
        (tr!("submodule-init"), tr!("submodule-init-hint"))
    } else {
        (tr!("submodule-update"), tr!("submodule-update-hint"))
    };
    let off = update_blocked(info).or_else(|| (!can_update).then(wait));
    if ui
        .add_enabled(off.is_none(), egui::Button::new(label))
        .on_hover_text(hint)
        .on_disabled_hover_text(off.unwrap_or_default())
        .clicked()
    {
        actions.push(Action::Submodule(submodule::Action::Update {
            path: Some(info.path.clone()),
            init,
        }));
        ui.close();
    }
    ui.separator();
    let not_here = init.then(|| tr!("submodule-not-checked-out"));
    if ui
        .add_enabled(
            not_here.is_none(),
            egui::Button::new(tr!("submodule-open-tab")),
        )
        .on_hover_text(tr!("submodule-open-tab-hint"))
        .on_disabled_hover_text(not_here.clone().unwrap_or_default())
        .clicked()
    {
        actions.push(Action::Tabs(crate::tabs::Action::OpenSubmodule(
            info.path.clone(),
        )));
        ui.close();
    }
    if file_view::can_launch(ui)
        && ui
            .add_enabled(
                not_here.is_none(),
                egui::Button::new(tr!("submodule-show-folder")),
            )
            .on_hover_text(tr!("submodule-show-folder-hint"))
            .on_disabled_hover_text(not_here.unwrap_or_default())
            .clicked()
    {
        actions.push(Action::Tabs(crate::tabs::Action::ShowSubmoduleFolder(
            info.path.clone(),
        )));
        ui.close();
    }
}

/// The notice under the bar after a command left submodules on another commit than the recorded
/// one (S62b): "Update" of exactly those and "Not now". `can_write`: no write or dialog in
/// progress.
pub(crate) fn outdated_notice(
    ui: &mut Ui,
    paths: &[project_domain::RepoPath],
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let warn = ui.visuals().warn_fg_color;
    let list = paths
        .iter()
        .map(|p| format!("“{p}”"))
        .collect::<Vec<_>>()
        .join(", ");
    egui::Frame::new()
        .fill(warn.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, warn.gamma_multiply(0.5)))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .outer_margin(egui::Margin::symmetric(0, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let hint = tr!("submodule-outdated-hint", list = list.as_str());
                ui.add(
                    egui::Label::new(
                        RichText::new(tr!("submodule-outdated", n = paths.len())).strong(),
                    )
                    .truncate(),
                )
                .on_hover_text(hint.as_str());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(tr!("submodule-outdated-dismiss"))
                        .on_hover_text(tr!("submodule-outdated-dismiss-hint"))
                        .clicked()
                    {
                        actions.push(Action::Submodule(submodule::Action::Dismiss));
                    }
                    let button = ui
                        .add_enabled(
                            can_write,
                            egui::Button::new(tr!("submodule-outdated-update")),
                        )
                        .on_hover_text(hint.as_str())
                        .on_disabled_hover_text(wait());
                    if button.clicked() {
                        actions.push(Action::Submodule(submodule::Action::UpdateOutdated));
                    }
                });
            });
        });
}
