use super::*;

/// Last component of the path, for the title.
pub(crate) fn repo_name(path: &str) -> &str {
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

/// The toolbar of the repo shown, GitKraken style: the branch on the left, the icon buttons
/// at the center. `writing`: a write is in progress, Undo, Redo,
/// Refresh, Fetch, Pull and Push wait. `network`: the fetch, pull or push in progress with its
/// latest progress. Open and the recent repos are in the "File" menu of the title bar.
pub(crate) fn top_bar(
    ui: &mut Ui,
    repo: &RepoView,
    writing: bool,
    network: Option<Network>,
    actions: &mut Vec<Action>,
) {
    let bar = ui.max_rect();
    ui.horizontal(|ui| {
        branch_info(ui, repo);
        // The width of the buttons is known only after drawing them: the last frame's centers them.
        let width_id = ui.scope_id().with("tools-width");
        let width = ui.data(|d| d.get_temp::<f32>(width_id)).unwrap_or(0.0);
        let space = bar.center().x - width / 2.0 - ui.cursor().min.x;
        ui.add_space(space.max(ui.spacing().item_spacing.x * 2.0));
        let start = ui.cursor().min.x;
        let can_write = !writing && !repo.modal_open();
        undo_buttons(ui, repo, writing, actions);
        ui.separator();
        let enabled = can_write && network.is_none();
        fetch_buttons(ui, repo, enabled, actions);
        offline_icon(ui, &repo.auto_fetch);
        sync_buttons(ui, repo, enabled, actions);
        ui.separator();
        stash_push_button(
            ui,
            repo,
            can_write,
            ToolButton::new(icons::STASH, &tr!("top-bar-stash")),
            actions,
        );
        refresh_button(ui, writing, actions);
        let width = ui.cursor().min.x - start - ui.spacing().item_spacing.x;
        ui.data_mut(|d| d.insert_temp(width_id, width));
        if let Some(network) = network {
            ui.separator();
            network_progress(ui, network, actions);
        }
    });
}

/// The branch under its caption (the repo is the tab's name).
fn branch_info(ui: &mut Ui, repo: &RepoView) {
    ui.add_space(4.0);
    caption(ui, &tr!("top-bar-branch"), |ui| {
        ui.horizontal(|ui| {
            ui.label(head_text(&repo.head));
            if let Some(upstream) = repo.status.as_ref().and_then(|s| s.upstream.as_ref()) {
                upstream_label(ui, upstream);
            }
        });
    });
}

/// `content` with `text` small above it.
fn caption(ui: &mut Ui, text: &str, content: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(RichText::new(text).small().weak());
        content(ui);
    });
}

/// A GitKraken-style button: the icon above, the name below, flat until the mouse is over it.
/// Disabled with [`Ui::add_enabled`]: then it is drawn faded.
pub(super) struct ToolButton<'a> {
    icon: &'a str,
    label: &'a str,
}

impl<'a> ToolButton<'a> {
    pub(super) fn new(icon: &'a str, label: &'a str) -> Self {
        Self { icon, label }
    }
}

impl egui::Widget for ToolButton<'_> {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        let color = ui.visuals().text_color();
        let icon = ui
            .painter()
            .layout_no_wrap(self.icon.to_owned(), icons::font(20.0), color);
        let font = FontId::proportional(11.0);
        let label = ui
            .painter()
            .layout_no_wrap(self.label.to_owned(), font, color);
        let width = (label.size().x.max(icon.size().x) + 14.0).max(48.0);
        let (rect, response) = ui.allocate_exact_size(vec2(width, 46.0), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), self.label)
        });
        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact(&response);
            if response.hovered() || response.has_focus() {
                ui.painter()
                    .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
            }
            let top = rect.top() + 5.0;
            let icon_pos = egui::pos2(rect.center().x - icon.size().x / 2.0, top);
            let label_pos = egui::pos2(
                rect.center().x - label.size().x / 2.0,
                rect.bottom() - 5.0 - label.size().y,
            );
            ui.painter().galley(icon_pos, icon, color);
            ui.painter().galley(label_pos, label, color);
        }
        response
    }
}

/// "Refresh": refs, status and log again.
fn refresh_button(ui: &mut Ui, writing: bool, actions: &mut Vec<Action>) {
    let button = ui
        .add_enabled(
            !writing,
            ToolButton::new(icons::REFRESH, &tr!("top-bar-refresh")),
        )
        .on_hover_text(tr!("top-bar-refresh-hint"))
        .on_disabled_hover_text(tr!("top-bar-after-operation"));
    if button.clicked() {
        actions.push(Action::Refresh);
    }
}

/// "Undo" and "Redo", with the journal entry they would affect on hover.
fn undo_buttons(ui: &mut Ui, repo: &RepoView, writing: bool, actions: &mut Vec<Action>) {
    // With an operation in progress (merge, rebase...) the engine refuses undo and redo.
    let in_operation = repo.status.as_ref().is_some_and(|s| s.operation.is_some());
    for op in [UndoOp::Undo, UndoOp::Redo] {
        let available = op.available(&repo.history);
        let enabled = available && !in_operation && !writing && !repo.modal_open();
        let icon = match op {
            UndoOp::Undo => icons::UNDO,
            UndoOp::Redo => icons::REDO,
        };
        let button = ui.add_enabled(enabled, ToolButton::new(icon, &op.label()));
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
}

/// A repo of the recent list, as the menus show it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecentRepo {
    pub(crate) path: String,
    /// Its folder no longer exists: shown greyed out, "Remove missing" drops it.
    pub(crate) missing: bool,
}

/// The menu of the recent repos: one opens in a tab (or shows the tab where it is open).
pub(super) fn recent_menu(ui: &mut Ui, recent: &[RecentRepo], actions: &mut Vec<Action>) {
    ui.add_enabled_ui(!recent.is_empty(), |ui| {
        ui.menu_button(tr!("top-bar-recent"), |ui| {
            for repo in recent {
                let button = ui
                    .add_enabled(!repo.missing, egui::Button::new(recent_text(repo)))
                    .on_disabled_hover_text(tr!("top-bar-recent-missing-hint", path = &repo.path));
                if button.clicked() {
                    actions.push(Action::Open(repo.path.clone()));
                    ui.close();
                }
            }
            if recent.iter().any(|r| r.missing) {
                ui.separator();
                if ui
                    .button(tr!("top-bar-recent-forget"))
                    .on_hover_text(tr!("top-bar-recent-forget-hint"))
                    .clicked()
                {
                    actions.push(Action::ForgetMissing);
                }
            }
        })
        .response
        .on_hover_text(tr!("top-bar-recent-hint"))
        .on_disabled_hover_text(tr!("top-bar-recent-none"));
    });
}

/// Name and path of a recent repo: two folders with the same name stay distinguishable.
fn recent_item(path: &str) -> String {
    tr!("top-bar-recent-item", name = repo_name(path), path = path)
}

/// [`recent_item`], greyed out if the folder is missing.
fn recent_text(repo: &RecentRepo) -> RichText {
    let text = RichText::new(recent_item(&repo.path));
    if repo.missing {
        text.weak().strikethrough()
    } else {
        text
    }
}

/// The "Fetch" button and, with several remotes, the menu to choose one.
fn fetch_buttons(ui: &mut Ui, repo: &RepoView, enabled: bool, actions: &mut Vec<Action>) {
    let button = ui
        .add_enabled(
            enabled,
            ToolButton::new(icons::FETCH, &tr!("top-bar-fetch")),
        )
        .on_hover_text(tr!("top-bar-fetch-hint"))
        .on_disabled_hover_text(wait());
    if button.clicked() {
        actions.push(Action::Fetch(None));
    }
    let remotes = repo.remotes.names();
    if remotes.len() > 1 {
        ui.add_enabled_ui(enabled, |ui| {
            let caret = RichText::new(icons::CARET_DOWN).font(icons::font(12.0));
            ui.menu_button(caret, |ui| {
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
fn sync_buttons(ui: &mut Ui, repo: &RepoView, enabled: bool, actions: &mut Vec<Action>) {
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
        .add_enabled(enabled, ToolButton::new(icons::PULL, &tr!("top-bar-pull")))
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
        .add_enabled(enabled, ToolButton::new(icons::PUSH, &tr!("top-bar-push")))
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
fn network_progress(ui: &mut Ui, network: Network, actions: &mut Vec<Action>) {
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
        .add_enabled(
            !network.cancelling,
            ToolButton::new(icons::STOP, &tr!("top-bar-stop")),
        )
        .on_hover_text(network.stop_hint)
        .on_disabled_hover_text(tr!("top-bar-stopping"));
    if stop.clicked() {
        actions.push(network.cancel);
    }
}

/// `write` is the write in progress, described: it takes precedence over the other commands. `notice`
/// is an informational message, shown if there is nothing in progress and no error. `repo`: the repo
/// shown, for the changed files and its path on the right.
pub(crate) fn status_bar(
    ui: &mut Ui,
    repo: Option<&RepoView>,
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
        // On the right what is always useful: the changed files and where the repo is.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            egui::warn_if_debug_build(ui);
            let Some(repo) = repo else {
                return;
            };
            let changes = repo.status.as_ref().map_or(0, |s| s.entries.len());
            if changes > 0 {
                ui.label(RichText::new(tr!("top-bar-changed-files", n = changes)).small());
                ui.separator();
            }
            ui.add(egui::Label::new(RichText::new(&repo.path).small().weak()).truncate());
        });
    });
}

/// The screen of a tab without a repo: open one, or one of the `recent`.
pub(crate) fn welcome(
    ui: &mut Ui,
    can_pick: bool,
    busy: bool,
    recent: &[RecentRepo],
    actions: &mut Vec<Action>,
) {
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
            create_buttons(ui, actions);
        } else {
            ui.heading(tr!("top-bar-no-repo"));
            ui.add_space(8.0);
            create_buttons(ui, actions);
        }
        if !busy && !recent.is_empty() {
            ui.add_space(16.0);
            ui.label(RichText::new(tr!("top-bar-recent-heading")).strong());
            for repo in recent {
                if repo.missing {
                    ui.label(recent_text(repo))
                        .on_hover_text(tr!("top-bar-recent-missing-hint", path = &repo.path));
                } else if ui.link(recent_item(&repo.path)).clicked() {
                    actions.push(Action::Open(repo.path.clone()));
                }
            }
            if recent.iter().any(|r| r.missing)
                && ui
                    .small_button(tr!("top-bar-recent-forget"))
                    .on_hover_text(tr!("top-bar-recent-forget-hint"))
                    .clicked()
            {
                actions.push(Action::ForgetMissing);
            }
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
