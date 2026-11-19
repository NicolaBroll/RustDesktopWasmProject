use super::*;
use crate::keys::{self, Command, with_shortcut};
use crate::{discard, fetch, status, sync, tabs};
use std::sync::Arc;

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
        RichText::new(short).color(theme::counter(ui.visuals()))
    };
    ui.label(text).on_hover_text(long);
}

/// The toolbar of the repo shown, GitKraken style: the branch on the left, the icon buttons
/// at the center. `writing`: a write is in progress, Undo, Redo,
/// Refresh, Fetch, Pull and Push wait. `network`: the fetch, pull or push in progress with its
/// latest progress. Open and the recent repos are in the "File" menu of the title bar.
///
/// The buttons are measured before they are drawn: the ones the bar has no room for (a narrow
/// window, a large UI scale) go, from the right, into the "»" menu at the end, never half out
/// of the window. The branch on the left keeps its room.
pub(crate) fn top_bar(
    ui: &mut Ui,
    repo: &RepoView,
    writing: bool,
    network: Option<Network>,
    actions: &mut Vec<Action>,
) {
    let bar = ui.max_rect();
    ui.horizontal(|ui| {
        branch_info(ui, repo, !writing && !repo.modal_open(), actions);
        let tools = tools(ui.ctx(), repo, writing, network.is_some());
        let gap = ui.spacing().item_spacing.x;
        let separator = SEPARATOR;
        let slots: Vec<Slot> = tools.iter().map(|tool| tool.slot(ui)).collect();
        // Where the buttons may start, and the room from there to the right edge.
        let left = ui.cursor().min.x + gap * 2.0;
        let room = bar.right() - left;
        let shown = tools_that_fit(&slots, gap, separator, MORE_WIDTH, room);
        // All of them: centered in the bar, as far as the branch and the edge allow.
        let start = if shown == tools.len() {
            let width = tools_width(&slots, gap, separator);
            (bar.center().x - width / 2.0)
                .min(bar.right() - width)
                .max(left)
        } else {
            left
        };
        ui.add_space(start - ui.cursor().min.x);
        for (i, tool) in tools[..shown].iter().enumerate() {
            if tool.group && i > 0 {
                ui.add(egui::Separator::default().spacing(separator));
            }
            tool.button(ui, actions);
        }
        if shown < tools.len() {
            more_menu(ui, &tools[shown..], actions);
        }
        // What was measured is what was drawn: otherwise a button could end out of the bar.
        debug_assert!(
            {
                let mut measured = row_width(&slots, shown, gap, separator);
                if shown < tools.len() {
                    measured += if shown > 0 { gap } else { 0.0 } + MORE_WIDTH;
                }
                let drawn = ui.cursor().min.x - gap - start;
                (measured - drawn).abs() < 0.5
            },
            "the toolbar is not as wide as measured"
        );
        if let Some(network) = network {
            ui.separator();
            network_progress(ui, network, actions);
        }
    });
}

/// The width a separator between two groups of the toolbar takes.
const SEPARATOR: f32 = 6.0;

/// The width of the "»" button.
const MORE_WIDTH: f32 = 30.0;

/// The width of the caret beside "Fetch" (the menu of the remotes).
const CARET_WIDTH: f32 = 20.0;

/// The height of the buttons of the toolbar.
const TOOL_HEIGHT: f32 = 46.0;

/// A button of the toolbar for the layout: how wide it is and whether a separator comes
/// before it (the first of a group).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slot {
    width: f32,
    group: bool,
}

/// The width of the first `n` of `slots` in a row: `gap` between two items, a `separator`
/// before each group but the first item.
fn row_width(slots: &[Slot], n: usize, gap: f32, separator: f32) -> f32 {
    let mut width = 0.0;
    for (i, slot) in slots[..n].iter().enumerate() {
        if i > 0 {
            width += gap;
            if slot.group {
                width += separator + gap;
            }
        }
        width += slot.width;
    }
    width
}

/// The width of the whole toolbar.
fn tools_width(slots: &[Slot], gap: f32, separator: f32) -> f32 {
    row_width(slots, slots.len(), gap, separator)
}

/// How many buttons of the toolbar, from the left, are drawn in `room`: all of them if they
/// fit, otherwise as many as leave room for the "»" menu (`more` wide) that takes the others.
/// Never a button that would end beyond `room`.
fn tools_that_fit(slots: &[Slot], gap: f32, separator: f32, more: f32, room: f32) -> usize {
    if tools_width(slots, gap, separator) <= room {
        return slots.len();
    }
    (0..slots.len())
        .rev()
        .find(|&n| {
            let before = row_width(slots, n, gap, separator);
            let gap = if n > 0 { gap } else { 0.0 };
            before + gap + more <= room
        })
        .unwrap_or(0)
}

/// A button of the toolbar as data: drawn as a [`ToolButton`] or, when the bar has no room for
/// it, as an entry of the "»" menu, with the same label, shortcut, state and reasons.
struct Tool {
    icon: &'static str,
    label: String,
    /// The count at the top right of the icon (`↓2`); in the menu, after the label.
    badge: Option<String>,
    enabled: bool,
    /// The hover text when enabled.
    hint: String,
    /// The hover text when disabled: why.
    disabled_hint: String,
    /// The shortcut, shown in the menu entry (the hint has it too).
    shortcut: Option<Command>,
    action: Action,
    /// The first of a group: a separator before it.
    group: bool,
    /// "Fetch" only: the remotes to choose from, with more than one.
    remotes: Vec<String>,
    /// "Fetch" only: the automatic fetch cannot reach the remote (the cloud on the button).
    offline: bool,
}

impl Tool {
    fn new(icon: &'static str, label: String, action: Action) -> Self {
        Self {
            icon,
            label,
            badge: None,
            enabled: true,
            hint: String::new(),
            disabled_hint: String::new(),
            shortcut: None,
            action,
            group: false,
            remotes: Vec::new(),
            offline: false,
        }
    }

    fn widget(&self) -> ToolButton<'_> {
        ToolButton::new(self.icon, &self.label).badge(self.badge.clone())
    }

    /// The room the button takes in the bar, the caret of its menu included.
    fn slot(&self, ui: &Ui) -> Slot {
        let mut width = self.widget().width(ui);
        if !self.remotes.is_empty() {
            width += ui.spacing().item_spacing.x + CARET_WIDTH;
        }
        Slot {
            width,
            group: self.group,
        }
    }

    /// The button in the bar.
    fn button(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let button = ui
            .add_enabled(self.enabled, self.widget())
            .on_hover_text(&self.hint)
            .on_disabled_hover_text(&self.disabled_hint);
        if self.offline {
            offline_badge(ui, button.rect);
        }
        if button.clicked() {
            actions.push(self.action.clone());
        }
        if !self.remotes.is_empty() {
            ui.add_enabled_ui(self.enabled, |ui| {
                let caret = icon_button(ui, icons::CARET_DOWN, 12.0, CARET_WIDTH)
                    .on_hover_text(tr!("top-bar-fetch-from-hint"));
                egui::Popup::menu(&caret).show(|ui| self.remote_entries(ui, actions));
            });
        }
    }

    /// The entry of the "»" menu: the label with its count, the shortcut at the right.
    fn entry(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let text = match &self.badge {
            Some(badge) => format!("{} {badge}", self.label),
            None => self.label.clone(),
        };
        let mut button = egui::Button::new(text);
        if let Some(command) = self.shortcut {
            button = button.shortcut_text(keys::shortcut_text(ui.ctx(), &command.shortcut()));
        }
        let button = ui
            .add_enabled(self.enabled, button)
            .on_hover_text(&self.hint)
            .on_disabled_hover_text(&self.disabled_hint);
        if button.clicked() {
            actions.push(self.action.clone());
            ui.close();
        }
        if !self.remotes.is_empty() {
            ui.add_enabled_ui(self.enabled, |ui| self.remote_entries(ui, actions));
        }
    }

    /// "Fetch from …", one entry per remote (none with a single remote).
    fn remote_entries(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        for remote in &self.remotes {
            if ui
                .button(tr!("top-bar-fetch-from", remote = remote))
                .clicked()
            {
                actions.push(Action::Fetch(fetch::Action::Fetch(Some(remote.clone()))));
                ui.close();
            }
        }
    }
}

/// A flat button of the toolbar with only a glyph, `width` wide: the caret of "Fetch", "»".
fn icon_button(ui: &mut Ui, glyph: &str, size: f32, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, TOOL_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), glyph));
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() || response.has_focus() {
            ui.painter()
                .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
        }
        theme::focus_ring(ui, &response, rect);
        let color = ui.visuals().text_color();
        let font = if glyph == MORE {
            FontId::proportional(size)
        } else {
            icons::font(size)
        };
        let galley = ui.painter().layout_no_wrap(glyph.to_owned(), font, color);
        let pos = rect.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, color);
    }
    response
}

/// The glyph of the menu with the buttons the toolbar has no room for.
const MORE: &str = "»";

/// The "»" menu at the end of a toolbar that does not fit: the buttons left out, as entries.
fn more_menu(ui: &mut Ui, tools: &[Tool], actions: &mut Vec<Action>) {
    let more = icon_button(ui, MORE, 18.0, MORE_WIDTH).on_hover_text(tr!("top-bar-more-hint"));
    egui::Popup::menu(&more).show(|ui| {
        for (i, tool) in tools.iter().enumerate() {
            if tool.group && i > 0 {
                ui.separator();
            }
            tool.entry(ui, actions);
        }
    });
}

/// The buttons of the toolbar, left to right: Undo, Redo | Fetch, Pull, Push | Stash…, Refresh.
/// `busy`: a fetch, pull or push is in progress.
fn tools(ctx: &egui::Context, repo: &RepoView, writing: bool, busy: bool) -> Vec<Tool> {
    let can_write = !writing && !repo.modal_open();
    let mut tools = undo_tools(ctx, repo, writing);
    let network = can_write && !busy;
    tools.push(fetch_tool(ctx, repo, network));
    tools.extend(sync_tools(ctx, repo, network));
    tools.push(stash_tool(repo, can_write));
    tools.push(refresh_tool(ctx, writing));
    tools
}

/// The branch under its caption (the repo is the tab's name); a detached HEAD in the warning
/// color, with "Create branch here…".
fn branch_info(ui: &mut Ui, repo: &RepoView, can_write: bool, actions: &mut Vec<Action>) {
    ui.add_space(4.0);
    caption(ui, &tr!("top-bar-branch"), |ui| {
        ui.horizontal(|ui| {
            if let Head::Detached(id) = &repo.head {
                let text = RichText::new(head_text(&repo.head))
                    .color(ui.visuals().warn_fg_color)
                    .strong();
                ui.label(text)
                    .on_hover_text(tr!("top-bar-head-detached-hint", id = id.short()));
                let button = ui
                    .add_enabled(
                        can_write,
                        egui::Button::new(tr!("top-bar-create-here")).small(),
                    )
                    .on_hover_text(tr!("top-bar-create-here-hint"))
                    .on_disabled_hover_text(wait());
                if button.clicked() {
                    // The name field of the dialog takes the keyboard only if nothing has it.
                    button.surrender_focus();
                    actions.push(Action::Branch(crate::branch::Action::CreateBranchHere));
                }
                return;
            }
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
    /// A small count at the top right of the icon (`↓2`), if any.
    badge: Option<String>,
}

impl<'a> ToolButton<'a> {
    pub(super) fn new(icon: &'a str, label: &'a str) -> Self {
        Self {
            icon,
            label,
            badge: None,
        }
    }

    /// The count at the top right of the icon.
    fn badge(mut self, badge: Option<String>) -> Self {
        self.badge = badge;
        self
    }
}

impl ToolButton<'_> {
    /// The icon and the label laid out, and the width of the button around them.
    fn layout(&self, ui: &Ui) -> (Arc<egui::Galley>, Arc<egui::Galley>, f32) {
        let color = ui.visuals().text_color();
        let icon = ui
            .painter()
            .layout_no_wrap(self.icon.to_owned(), icons::font(20.0), color);
        let font = theme::small();
        let label = ui
            .painter()
            .layout_no_wrap(self.label.to_owned(), font, color);
        let width = (label.size().x.max(icon.size().x) + 14.0).max(48.0);
        (icon, label, width)
    }

    /// The width the button takes: known before drawing it, to lay out the toolbar.
    fn width(&self, ui: &Ui) -> f32 {
        self.layout(ui).2
    }
}

impl egui::Widget for ToolButton<'_> {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        let color = ui.visuals().text_color();
        let (icon, label, width) = self.layout(ui);
        let (rect, response) = ui.allocate_exact_size(vec2(width, TOOL_HEIGHT), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), self.label)
        });
        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact(&response);
            if response.hovered() || response.has_focus() {
                ui.painter()
                    .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
            }
            // The fill alone is the look of the mouse over the button.
            theme::focus_ring(ui, &response, rect);
            let top = rect.top() + 5.0;
            let icon_pos = egui::pos2(rect.center().x - icon.size().x / 2.0, top);
            let label_pos = egui::pos2(
                rect.center().x - label.size().x / 2.0,
                rect.bottom() - 5.0 - label.size().y,
            );
            let icon_right = icon_pos.x + icon.size().x;
            ui.painter().galley(icon_pos, icon, color);
            ui.painter().galley(label_pos, label, color);
            if let Some(badge) = self.badge {
                let accent = ui.visuals().selection.bg_fill;
                let text = ui.visuals().selection.stroke.color;
                let galley = ui.painter().layout_no_wrap(badge, theme::small(), text);
                let size = galley.size() + vec2(6.0, 0.0);
                let badge_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        (icon_right - 4.0).min(rect.right() - size.x),
                        rect.top() + 1.0,
                    ),
                    size,
                );
                ui.painter().rect_filled(badge_rect, size.y / 2.0, accent);
                ui.painter()
                    .galley(badge_rect.min + vec2(3.0, 0.0), galley, text);
            }
        }
        response
    }
}

/// "Refresh": refs, status and log again.
fn refresh_tool(ctx: &egui::Context, writing: bool) -> Tool {
    Tool {
        enabled: !writing,
        hint: with_shortcut(ctx, &tr!("top-bar-refresh-hint"), Command::Refresh),
        disabled_hint: tr!("top-bar-after-operation"),
        shortcut: Some(Command::Refresh),
        ..Tool::new(icons::REFRESH, tr!("top-bar-refresh"), Action::Refresh)
    }
}

/// "Stash…": opens the stash dialog, if there are changes and no write in progress (the "+"
/// of the Stashes section is [`stash_push_button`], with the same texts).
fn stash_tool(repo: &RepoView, can_write: bool) -> Tool {
    let changes = repo.has_changes();
    Tool {
        enabled: can_write && changes,
        hint: tr!("stash-push-help"),
        disabled_hint: if changes {
            wait()
        } else {
            tr!("stash-no-changes")
        },
        group: true,
        ..Tool::new(
            icons::STASH,
            tr!("top-bar-stash"),
            Action::Stash(crate::stash::Action::Open),
        )
    }
}

/// Whether "Undo" or "Redo" (button or shortcut) can run: with an operation in progress (merge,
/// rebase...) the engine refuses them.
pub(crate) fn undo_enabled(repo: &RepoView, op: UndoOp, writing: bool) -> bool {
    let in_operation = repo.status.as_ref().is_some_and(|s| s.operation.is_some());
    op.available(&repo.history) && !in_operation && !writing && !repo.modal_open()
}

/// The branch HEAD is on, with at least one commit: Pull and Push need it.
pub(crate) fn current_branch(head: &Head) -> Option<&str> {
    match head {
        Head::Branch {
            name,
            target: Some(_),
        } => Some(name.as_str()),
        _ => None,
    }
}

/// "Undo" and "Redo", with the journal entry they would affect on hover.
fn undo_tools(ctx: &egui::Context, repo: &RepoView, writing: bool) -> Vec<Tool> {
    let in_operation = repo.status.as_ref().is_some_and(|s| s.operation.is_some());
    let tool = |op: UndoOp| {
        let available = op.available(&repo.history);
        let enabled = undo_enabled(repo, op, writing);
        let (icon, command) = match op {
            UndoOp::Undo => (icons::UNDO, Command::Undo),
            UndoOp::Redo => (icons::REDO, Command::Redo),
        };
        let hint = op.hint(&repo.history);
        let disabled_hint = if available && in_operation {
            tr!("top-bar-undo-in-operation", hint = &hint)
        } else if available && !enabled {
            tr!("top-bar-undo-after-operation", hint = &hint)
        } else {
            hint.clone()
        };
        Tool {
            enabled,
            hint: with_shortcut(ctx, &hint, command),
            disabled_hint,
            shortcut: Some(command),
            ..Tool::new(icon, op.label(), Action::Discard(discard::Action::Undo(op)))
        }
    };
    vec![tool(UndoOp::Undo), tool(UndoOp::Redo)]
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
                    actions.push(Action::Tabs(tabs::Action::Open(repo.path.clone())));
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
                    actions.push(Action::Tabs(tabs::Action::ForgetMissing));
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
fn fetch_tool(ctx: &egui::Context, repo: &RepoView, enabled: bool) -> Tool {
    let mut hint = with_shortcut(ctx, &tr!("top-bar-fetch-hint"), Command::Fetch);
    hint.push('\n');
    hint.push_str(&last_fetch_text(repo.auto_fetch.last_fetch()));
    // The automatic fetch that cannot reach the remote: a badge on the button and the reason in
    // its tooltip, also while the button waits.
    let offline = offline_hint(&repo.auto_fetch);
    let mut waiting = wait();
    if let Some(offline) = &offline {
        for text in [&mut hint, &mut waiting] {
            text.push_str("\n\n");
            text.push_str(offline);
        }
    }
    let remotes = repo.remotes.names();
    Tool {
        enabled,
        hint,
        disabled_hint: waiting,
        shortcut: Some(Command::Fetch),
        group: true,
        remotes: if remotes.len() > 1 {
            remotes
        } else {
            Vec::new()
        },
        offline: offline.is_some(),
        ..Tool::new(
            icons::FETCH,
            tr!("top-bar-fetch"),
            Action::Fetch(fetch::Action::Fetch(None)),
        )
    }
}

/// "Last fetch: 3 min ago", or that there was none since the repo was opened.
fn last_fetch_text(last: Option<std::time::Instant>) -> String {
    match last {
        Some(at) => {
            let secs = i64::try_from(at.elapsed().as_secs()).unwrap_or(i64::MAX);
            tr!("top-bar-last-fetch", when = format::ago(secs))
        }
        None => tr!("top-bar-last-fetch-none"),
    }
}

/// The badges of Pull and Push: the commits to pull (`↓n`) and to push (`↑n`) compared to the
/// upstream, nothing for a zero or a gone upstream.
fn sync_badges(upstream: Option<&Upstream>) -> (Option<String>, Option<String>) {
    let count = |n: Option<u32>, arrow: char| n.filter(|&n| n > 0).map(|n| format!("{arrow}{n}"));
    let Some(up) = upstream else {
        return (None, None);
    };
    (count(up.behind, '↓'), count(up.ahead, '↑'))
}

/// The "Pull" and "Push" buttons of the active branch. Never a forced push from here: force is
/// offered only in the details of a rejected push.
fn sync_tools(ctx: &egui::Context, repo: &RepoView, enabled: bool) -> [Tool; 2] {
    let branch = current_branch(&repo.head);
    let tracked = repo.status.as_ref().and_then(|s| s.upstream.as_ref());
    let (pull_badge, push_badge) = sync_badges(tracked.filter(|_| branch.is_some()));
    let upstream = tracked.map(|u| u.name.as_str());
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
    // Disabled on a branch: it waits; without one: the reason is the hint itself.
    let tool = |icon, label, badge, hint: String, command, action| Tool {
        badge,
        enabled,
        disabled_hint: if branch.is_some() {
            wait()
        } else {
            hint.clone()
        },
        hint: with_shortcut(ctx, &hint, command),
        shortcut: Some(command),
        ..Tool::new(icon, label, Action::Sync(action))
    };
    [
        tool(
            icons::PULL,
            tr!("top-bar-pull"),
            pull_badge,
            pull_hint,
            Command::Pull,
            sync::Action::Pull(None, None),
        ),
        tool(
            icons::PUSH,
            tr!("top-bar-push"),
            push_badge,
            push_hint,
            Command::Push,
            sync::Action::Push(None),
        ),
    ]
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

/// A read shorter than this does not reach the status bar: the automatic refresh on focus would
/// make it flash.
const QUIET_READ: std::time::Duration = std::time::Duration::from_millis(400);

/// The command to show as in progress: a write at once, a read once it has run for
/// [`QUIET_READ`] (a repaint is asked for that moment).
fn slow_job<'a>(ui: &Ui, running: &'a [Job]) -> Option<&'a Job> {
    let now = std::time::Instant::now();
    let shown = running
        .iter()
        .find(|job| job.writes || now.duration_since(job.started) >= QUIET_READ);
    if shown.is_none() && !running.is_empty() {
        ui.ctx().request_repaint_after(QUIET_READ);
    }
    shown
}

/// The share of the bar a message that needs it always gets: the repo path on the right is
/// cut to leave it.
const MESSAGE_SHARE: f32 = 0.6;

/// How the message of the status bar is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    /// A command in progress or a fresh notice.
    Plain,
    /// A notice older than [`status::FRESH`].
    Old,
    Error,
}

/// The message of the status bar: `text` whole (the hover text), of which the bar draws one line.
struct StatusText {
    text: String,
    spinner: bool,
    tone: Tone,
}

impl StatusText {
    /// The line on the bar: the first one of a notice (the backup refs follow it), an error
    /// with its lines joined.
    fn line(&self) -> std::borrow::Cow<'_, str> {
        match self.tone {
            Tone::Error => status::one_line(&self.text),
            Tone::Plain | Tone::Old => status::line(&self.text).into(),
        }
    }

    /// The width the line takes when nothing cuts it, spinner included.
    fn width(&self, ui: &Ui) -> f32 {
        let spinner = if self.spinner {
            ui.spacing().interact_size.y + ui.spacing().item_spacing.x
        } else {
            0.0
        };
        spinner + text_width(ui, &self.line())
    }

    /// One line, cut with "…" where the room ends; the whole text on hover if the line does not
    /// say it all, and in the clipboard from the context menu.
    fn show(&self, ui: &mut Ui) {
        if self.spinner {
            ui.spinner();
        }
        let line = self.line();
        let cut = text_width(ui, &line) > ui.available_width();
        let text = RichText::new(&*line);
        let text = match self.tone {
            Tone::Plain => text,
            Tone::Old => text.weak(),
            Tone::Error => text.color(ui.visuals().error_fg_color),
        };
        let label = ui.add(
            egui::Label::new(text)
                .truncate()
                .show_tooltip_when_elided(false)
                // The click is for the context menu, which the keyboard does not open: not a
                // Tab stop.
                .sense(Sense::CLICK),
        );
        // The hover text cannot be selected: the menu copies it whole, backup refs included.
        let whole = self.text.trim_end();
        label.context_menu(|ui| {
            if ui.button(tr!("status-copy")).clicked() {
                ui.ctx().copy_text(whole.to_owned());
                ui.close();
            }
        });
        if cut || line.len() < whole.len() {
            label.on_hover_text(whole);
        }
    }
}

/// The width of `text` on one line of the body font.
fn text_width(ui: &Ui, text: &str) -> f32 {
    let wrap = Some(egui::TextWrapMode::Extend);
    egui::WidgetText::from(text)
        .into_galley(ui, wrap, f32::INFINITY, egui::TextStyle::Body)
        .size()
        .x
}

/// What the status bar says at `now`: the command in progress, else the error, else the notice
/// (weak once it is old: a repaint is asked for that moment).
fn status_text(
    ui: &Ui,
    running: &[Job],
    write: Option<&str>,
    error: &str,
    notice: &status::Notice,
    now: std::time::Instant,
) -> Option<StatusText> {
    let (text, spinner, tone) = if let Some(write) = write {
        (tr!("top-bar-running", what = write), true, Tone::Plain)
    } else if let Some(job) = slow_job(ui, running) {
        let text = match running.len() - 1 {
            0 => tr!("top-bar-running", what = &job.command),
            n => tr!("top-bar-running-queued", what = &job.command, n = n),
        };
        (text, true, Tone::Plain)
    } else if !error.is_empty() {
        (error.to_owned(), false, Tone::Error)
    } else if !notice.is_empty() {
        let tone = match notice.fresh_for(now) {
            Some(left) => {
                ui.ctx().request_repaint_after(left);
                Tone::Plain
            }
            None => Tone::Old,
        };
        (notice.to_string(), false, tone)
    } else {
        return None;
    };
    Some(StatusText {
        text,
        spinner,
        tone,
    })
}

/// `write` is the write in progress, described: it takes precedence over the other commands. `notice`
/// is an informational message, shown if there is nothing in progress and no error. `repo`: the repo
/// shown, for its path on the right. The message is one line: the items on the right are laid out
/// first and it is cut in the room they leave, which is at least [`MESSAGE_SHARE`] of the bar.
pub(crate) fn status_bar(
    ui: &mut Ui,
    repo: Option<&RepoView>,
    running: &[Job],
    write: Option<&str>,
    error: &str,
    notice: &status::Notice,
) {
    let message = status_text(ui, running, write, error, notice, std::time::Instant::now());
    ui.horizontal(|ui| {
        let width = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        let wanted = message.as_ref().map_or(0.0, |m| m.width(ui) + gap);
        // What the message does not need, and never more than what its share leaves.
        let right = (width - wanted).max(width * (1.0 - MESSAGE_SHARE));
        // On the right where the repo is (the changed files are on the graph's first row).
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            egui::warn_if_debug_build(ui);
            if let Some(repo) = repo {
                let room = (right - (width - ui.available_width())).max(0.0);
                let size = vec2(room, ui.available_height());
                ui.allocate_ui_with_layout(size, *ui.layout(), |ui| {
                    ui.add(egui::Label::new(RichText::new(&repo.path).small().weak()).truncate());
                });
            }
            if let Some(message) = &message {
                let left = egui::Layout::left_to_right(egui::Align::Center);
                ui.with_layout(left, |ui| message.show(ui));
            }
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
                actions.push(Action::Tabs(tabs::Action::PickFolder));
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
                } else {
                    // The name is the link, the path under it tells apart two folders with the same name.
                    if ui.link(repo_name(&repo.path)).clicked() {
                        actions.push(Action::Tabs(tabs::Action::Open(repo.path.clone())));
                    }
                    ui.label(RichText::new(&repo.path).small().weak());
                    ui.add_space(4.0);
                }
            }
            if recent.iter().any(|r| r.missing)
                && ui
                    .small_button(tr!("top-bar-recent-forget"))
                    .on_hover_text(tr!("top-bar-recent-forget-hint"))
                    .clicked()
            {
                actions.push(Action::Tabs(tabs::Action::ForgetMissing));
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

    /// Undo, Redo | Fetch, Pull, Push | Stash…, Refresh: 50 wide each, 8 between two, 6 for a
    /// separator, 30 for "»".
    fn seven() -> Vec<Slot> {
        [false, false, true, false, false, true, false]
            .map(|group| Slot { width: 50.0, group })
            .to_vec()
    }

    fn fit(room: f32) -> usize {
        tools_that_fit(&seven(), 8.0, 6.0, 30.0, room)
    }

    #[test]
    fn the_toolbar_keeps_the_buttons_that_fit_and_the_rest_goes_in_the_menu() {
        // 7 buttons, 6 gaps and 2 separators with their own gap.
        let whole = 7.0 * 50.0 + 6.0 * 8.0 + 2.0 * (6.0 + 8.0);
        assert_eq!(tools_width(&seven(), 8.0, 6.0), whole);
        assert_eq!(whole, 426.0);
        // Room for all: no menu, also when it fits to the point.
        assert_eq!(fit(1000.0), 7);
        assert_eq!(fit(whole), 7);
        // One point short: "Refresh" goes into the menu. The six before it take
        // 426 - 58 = 368, with the menu 368 + 8 + 30 = 406.
        assert_eq!(fit(whole - 1.0), 6);
        assert_eq!(fit(406.0), 6);
        // Five end at 296 (the separator before "Stash…" is not drawn): with the menu 334.
        assert_eq!(fit(405.0), 5);
        assert_eq!(fit(334.0), 5);
        assert_eq!(fit(333.0), 4);
        // Undo and Redo take 108, the menu ends at 146; one button at 50, the menu at 88.
        assert_eq!(fit(146.0), 2);
        assert_eq!(fit(145.0), 1);
        assert_eq!(fit(88.0), 1);
        // Only the menu, even when there is no room for it either: every command is in it.
        assert_eq!(fit(87.0), 0);
        assert_eq!(fit(30.0), 0);
        assert_eq!(fit(0.0), 0);
        assert_eq!(fit(-20.0), 0);
    }

    #[test]
    fn what_the_toolbar_draws_never_ends_beyond_its_room() {
        let slots = seven();
        for room in 0..500 {
            let room = room as f32;
            let shown = tools_that_fit(&slots, 8.0, 6.0, 30.0, room);
            let mut width = row_width(&slots, shown, 8.0, 6.0);
            if shown < slots.len() {
                width += if shown > 0 { 8.0 } else { 0.0 } + 30.0;
            }
            // Below the width of the menu alone nothing can fit: the menu is drawn anyway.
            assert!(
                width <= room.max(30.0),
                "{shown} buttons in {room}: {width}"
            );
        }
    }

    #[test]
    fn pull_and_push_badges_count_the_upstream_commits() {
        let up = |ahead, behind| Upstream {
            name: "origin/main".to_owned(),
            ahead,
            behind,
        };
        assert_eq!(
            sync_badges(Some(&up(Some(1), Some(1)))),
            (Some("↓1".to_owned()), Some("↑1".to_owned()))
        );
        assert_eq!(
            sync_badges(Some(&up(Some(2), Some(0)))),
            (None, Some("↑2".to_owned()))
        );
        assert_eq!(sync_badges(Some(&up(None, None))), (None, None));
        assert_eq!(sync_badges(None), (None, None));
    }
}
