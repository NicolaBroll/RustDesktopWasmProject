use super::*;

/// Last component of the path, for the title.
pub(super) fn repo_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

pub(super) fn head_text(head: &Head) -> String {
    match head {
        Head::Branch { name, target: None } => tr!("top-bar-head-unborn", name = name),
        Head::Branch { name, .. } => name.clone(),
        Head::Detached(id) => tr!("top-bar-head-detached", id = id.short()),
    }
}

/// Ahead and behind the upstream (`↑2 ↓1`, `upstream gone`), with the explanation
/// on hover; nothing if the branch is aligned.
pub(super) fn upstream_label(ui: &mut Ui, upstream: &Upstream) {
    let (short, long) = remotes::ahead_behind(upstream);
    if short.is_empty() {
        return;
    }
    let text = if upstream.ahead.is_none() {
        RichText::new(short)
            .color(ui.visuals().warn_fg_color)
            .small()
    } else {
        RichText::new(short).weak()
    };
    ui.label(text).on_hover_text(long);
}

/// `writing`: a write is in progress, Undo, Redo, Refresh, Fetch, Pull and Push
/// wait. `network`: the fetch, pull or push in progress with its latest progress.
pub(crate) fn top_bar(
    ui: &mut Ui,
    repo: Option<&RepoView>,
    can_pick: bool,
    writing: bool,
    network: Option<Network>,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        repo_bar(ui, repo, can_pick, writing, network, actions);
    });
}

fn repo_bar(
    ui: &mut Ui,
    repo: Option<&RepoView>,
    can_pick: bool,
    writing: bool,
    network: Option<Network>,
    actions: &mut Vec<Action>,
) {
    {
        if can_pick && ui.button(tr!("top-bar-open")).clicked() {
            actions.push(Action::PickFolder);
        }
        let Some(repo) = repo else {
            return;
        };
        let button = ui
            .add_enabled(!writing, egui::Button::new(tr!("top-bar-refresh")))
            .on_hover_text(tr!("top-bar-refresh-hint"))
            .on_disabled_hover_text(tr!("top-bar-after-operation"));
        if button.clicked() {
            actions.push(Action::Refresh);
        }
        ui.separator();
        match network {
            Some(network) => network_progress(ui, network, actions),
            None => {
                let enabled = !writing && !repo.modal_open();
                fetch_buttons(ui, repo, enabled, actions);
                sync_buttons(ui, repo, enabled, actions);
                stash_push_button(ui, repo, enabled, &tr!("top-bar-stash"), actions);
            }
        }
        ui.separator();
        // With an operation in progress (merge, rebase...) the engine refuses undo and redo.
        let in_operation = repo.status.as_ref().is_some_and(|s| s.operation.is_some());
        for op in [UndoOp::Undo, UndoOp::Redo] {
            let available = op.available(&repo.history);
            let enabled = available && !in_operation && !writing && !repo.modal_open();
            let button = ui.add_enabled(enabled, egui::Button::new(op.label()));
            let hint = op.hint(&repo.history);
            let button = if available && in_operation {
                button.on_disabled_hover_text(tr!("top-bar-undo-in-operation", hint = &hint))
            } else if available && !enabled {
                button.on_disabled_hover_text(tr!("top-bar-undo-after-operation", hint = &hint))
            } else {
                button.on_hover_text(&hint).on_disabled_hover_text(&hint)
            };
            if button.clicked() {
                actions.push(Action::Undo(op));
            }
        }
        ui.separator();
        ui.strong(repo_name(&repo.path)).on_hover_text(&repo.path);
        ui.separator();
        ui.label(head_text(&repo.head));
        if let Some(status) = &repo.status {
            if let Some(upstream) = &status.upstream {
                upstream_label(ui, upstream);
            }
            let changes = status.entries.len();
            if changes > 0 {
                ui.separator();
                ui.label(tr!("top-bar-changed-files", n = changes));
            }
        }
    }
}

/// The "Fetch" button and, with several remotes, the menu to choose one.
pub(super) fn fetch_buttons(
    ui: &mut Ui,
    repo: &RepoView,
    enabled: bool,
    actions: &mut Vec<Action>,
) {
    let button = ui
        .add_enabled(enabled, egui::Button::new(tr!("top-bar-fetch")))
        .on_hover_text(tr!("top-bar-fetch-hint"))
        .on_disabled_hover_text(wait());
    if button.clicked() {
        actions.push(Action::Fetch(None));
    }
    let remotes = repo.remotes.names();
    if remotes.len() > 1 {
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button(tr!("top-bar-fetch-from-menu"), |ui| {
                for remote in remotes {
                    if ui
                        .button(tr!("top-bar-fetch-from", remote = &remote))
                        .clicked()
                    {
                        actions.push(Action::Fetch(Some(remote)));
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text(tr!("top-bar-fetch-from-hint"));
        });
    }
}

/// The "Pull" and "Push" buttons of the active branch. Never a forced push from here: force is
/// offered only in the window of a rejected push.
pub(super) fn sync_buttons(ui: &mut Ui, repo: &RepoView, enabled: bool, actions: &mut Vec<Action>) {
    let branch = match &repo.head {
        Head::Branch {
            name,
            target: Some(_),
        } => Some(name.as_str()),
        _ => None,
    };
    let upstream = repo
        .status
        .as_ref()
        .and_then(|s| s.upstream.as_ref())
        .map(|u| u.name.as_str());
    let (pull_hint, push_hint) = match (branch, upstream) {
        (None, _) => (tr!("top-bar-detached-hint"), tr!("top-bar-detached-hint")),
        (Some(b), Some(up)) => (
            tr!("top-bar-pull-hint", upstream = up, branch = b),
            tr!("top-bar-push-hint", upstream = up, branch = b),
        ),
        (Some(b), None) => (
            tr!("top-bar-pull-no-upstream-hint", branch = b),
            tr!("top-bar-push-first-hint", branch = b),
        ),
    };
    let enabled = enabled && branch.is_some();
    let pull = ui
        .add_enabled(enabled, egui::Button::new(tr!("top-bar-pull")))
        .on_hover_text(&pull_hint);
    let pull = if branch.is_some() {
        pull.on_disabled_hover_text(wait())
    } else {
        pull.on_disabled_hover_text(&pull_hint)
    };
    if pull.clicked() {
        actions.push(Action::Pull(None, None));
    }
    let push = ui
        .add_enabled(enabled, egui::Button::new(tr!("top-bar-push")))
        .on_hover_text(&push_hint);
    let push = if branch.is_some() {
        push.on_disabled_hover_text(wait())
    } else {
        push.on_disabled_hover_text(&push_hint)
    };
    if push.clicked() {
        actions.push(Action::Push(None));
    }
}

/// The fetch, pull or push in progress: what, phase, bar (if git gives the percentage) and
/// "Stop".
pub(super) fn network_progress(ui: &mut Ui, network: Network, actions: &mut Vec<Action>) {
    ui.spinner();
    ui.strong(&network.what);
    let progress = network.progress.as_ref();
    let (text, fraction) = match progress {
        Some(p) => fetch::describe(p),
        None => (tr!("top-bar-connecting"), None),
    };
    let label = ui.label(text);
    if let Some(p) = progress {
        label.on_hover_text(&p.text);
    }
    if let Some(fraction) = fraction {
        ui.add(
            egui::ProgressBar::new(fraction)
                .desired_width(120.0)
                .show_percentage(),
        );
    }
    let stop = ui
        .add_enabled(!network.cancelling, egui::Button::new(tr!("top-bar-stop")))
        .on_hover_text(network.stop_hint)
        .on_disabled_hover_text(tr!("top-bar-stopping"));
    if stop.clicked() {
        actions.push(network.cancel);
    }
}

/// `write` is the write in progress, described: it takes precedence over the other commands. `notice`
/// is an informational message, shown if there is nothing in progress and no error.
pub(crate) fn status_bar(
    ui: &mut Ui,
    running: &[Job],
    write: Option<&str>,
    error: &str,
    notice: &str,
) {
    ui.horizontal(|ui| {
        if let Some(write) = write {
            ui.spinner();
            ui.label(tr!("top-bar-running", what = write));
        } else if let Some(job) = running.first() {
            ui.spinner();
            let text = match running.len() - 1 {
                0 => tr!("top-bar-running", what = &job.command),
                n => tr!("top-bar-running-queued", what = &job.command, n = n),
            };
            ui.label(text);
        } else if !error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        } else if !notice.is_empty() {
            ui.label(notice);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            egui::warn_if_debug_build(ui);
        });
    });
}

pub(crate) fn welcome(ui: &mut Ui, can_pick: bool, busy: bool, actions: &mut Vec<Action>) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 3.0);
        if busy {
            ui.spinner();
            ui.label(tr!("top-bar-opening"));
        } else if can_pick {
            ui.heading(tr!("top-bar-no-repo"));
            ui.add_space(8.0);
            if ui.button(tr!("top-bar-open-repo")).clicked() {
                actions.push(Action::PickFolder);
            }
        } else {
            ui.heading(tr!("top-bar-no-repo"));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_name_is_the_last_component() {
        assert_eq!(repo_name("/home/ada/project"), "project");
        assert_eq!(repo_name("/home/ada/project/"), "project");
        assert_eq!(repo_name(r"C:\repo\app"), "app");
        assert_eq!(repo_name("example"), "example");
        assert_eq!(repo_name("/"), "/");
    }
}
