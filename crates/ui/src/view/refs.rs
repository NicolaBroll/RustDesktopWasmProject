use super::*;
use crate::ref_tree::{self, Row};
use crate::{branch, merge, remotes, sync};
use egui::{Rect, UiBuilder};
use project_domain::{HEADS_PREFIX, remote_short};
use std::collections::HashSet;

/// Height of a row of the column: all equal, so only the rows in view are drawn.
pub(super) const ROW: f32 = 20.0;
/// Indentation of a tree level.
const INDENT: f32 = 14.0;
/// Width of the caret of a folder (and the space before a ref at the same level).
const CARET: f32 = 14.0;

/// What the column remembers between frames, per tab: the filter, the folders closed by the user
/// and the trees built for them, rebuilt only when the refs, the remotes, the filter or the
/// folders change.
/// `Clone` only because egui's memory asks for it: the state is taken and put back, never cloned.
#[derive(Clone, Default)]
struct ColumnState {
    filter: String,
    /// `<section>:<path>` of the closed folders.
    closed: HashSet<String>,
    /// Grows at every open or close.
    toggles: u64,
    trees: Option<Trees>,
}

/// The rows of the three ref sections and what they were built from.
#[derive(Clone)]
struct Trees {
    key: (u64, String, u64, Vec<String>),
    sections: [Section; 3],
}

/// The rows of a section; `refs` maps the rows' indices to `RepoView::refs`.
#[derive(Clone, Default)]
struct Section {
    refs: Vec<usize>,
    rows: Vec<Row>,
    /// Refs matching the filter (all without a filter).
    matches: usize,
}

/// The sections, in their order, with what tells them apart.
const SECTIONS: [(RefKind, &str); 3] = [
    (RefKind::LocalBranch, "ref-local"),
    (RefKind::RemoteBranch, "ref-remote"),
    (RefKind::Tag, "ref-tag"),
];

/// The left column: a filter, then local branches, remotes with their branches, tags and stashes.
/// `writing`: a write is in progress, the menu entries that write wait.
pub(crate) fn refs_panel(ui: &mut Ui, repo: &RepoView, writing: bool, actions: &mut Vec<Action>) {
    let state_id = ui.make_persistent_id("refs-column");
    let mut state: ColumnState =
        ui.data_mut(|d| std::mem::take(d.get_temp_mut_or_default(state_id)));
    filter_box(ui, &mut state.filter);
    let filter = state.filter.trim().to_lowercase();
    let remotes: Vec<String> = repo
        .remotes
        .list()
        .unwrap_or_default()
        .iter()
        .map(|r| r.name.clone())
        .collect();
    let key = (repo.refs_version, filter.clone(), state.toggles, remotes);
    if state.trees.as_ref().is_none_or(|t| t.key != key) {
        state.trees = Some(build_trees(repo, &filter, &state.closed, key));
    }
    let Some(trees) = state.trees.take() else {
        return;
    };
    let current = match &repo.head {
        Head::Branch { name, .. } => Some(format!("{HEADS_PREFIX}{name}")),
        Head::Detached(_) => None,
    };
    let column = Column {
        repo,
        current: current.as_deref(),
        can_write: !writing && !repo.modal_open(),
        merge: merge::Gate::new(&repo.head, repo.status.as_ref()),
        filtering: !filter.is_empty(),
    };
    let mut toggled = Vec::new();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let [local, remote, tags] = &trees.sections;
            if !column.filtering || local.matches > 0 {
                let title = tr!(
                    "refs-section",
                    title = tr!("refs-local-branches"),
                    n = local.matches
                );
                section(
                    ui,
                    "ref-local",
                    icons::BRANCH,
                    &title,
                    true,
                    |_| {},
                    |ui| {
                        column.rows(ui, "ref-local", local, &mut toggled, actions);
                    },
                );
            }
            remotes_section(ui, &column, remote, &mut toggled, actions);
            if !column.filtering || tags.matches > 0 {
                let title = tr!("refs-section", title = tr!("refs-tags"), n = tags.matches);
                section(
                    ui,
                    "ref-tag",
                    icons::TAG,
                    &title,
                    false,
                    |_| {},
                    |ui| {
                        column.rows(ui, "ref-tag", tags, &mut toggled, actions);
                    },
                );
            }
            if !column.filtering {
                stash_section(ui, repo, column.can_write, actions);
            }
        });
    state.trees = Some(trees);
    for key in toggled {
        if !state.closed.remove(&key) {
            state.closed.insert(key);
        }
        state.toggles += 1;
    }
    ui.data_mut(|d| d.insert_temp(state_id, state));
}

/// The filter field, with "×" to empty it.
fn filter_box(ui: &mut Ui, filter: &mut String) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(icons::text(icons::SEARCH, 14.0).weak());
        let clear = !filter.is_empty();
        let width = ui.available_width() - if clear { 24.0 } else { 0.0 };
        let edit = egui::TextEdit::singleline(filter)
            .hint_text(tr!("refs-filter-hint"))
            .desired_width(width);
        let response = ui.add(edit);
        if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            filter.clear();
        }
        if clear
            && ui
                .add(egui::Button::new(icons::text(icons::CLOSE, 12.0)).frame(false))
                .on_hover_text(tr!("refs-filter-clear"))
                .clicked()
        {
            filter.clear();
        }
    });
    ui.add_space(2.0);
}

fn build_trees(
    repo: &RepoView,
    filter: &str,
    closed: &HashSet<String>,
    key: (u64, String, u64, Vec<String>),
) -> Trees {
    let sections = SECTIONS.map(|(kind, salt)| {
        let refs: Vec<usize> = (0..repo.refs.len())
            .filter(|&i| repo.refs[i].kind == kind)
            .collect();
        let infos: Vec<&RefInfo> = refs.iter().map(|&i| &repo.refs[i]).collect();
        let prefix = format!("{salt}:");
        let closed: HashSet<String> = closed
            .iter()
            .filter_map(|c| c.strip_prefix(&prefix).map(str::to_owned))
            .collect();
        let roots: Vec<&str> = match kind {
            RefKind::RemoteBranch => key.3.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        };
        let rows = ref_tree::rows(&infos, &roots, filter, &closed, kind == RefKind::Tag);
        let matches = if filter.is_empty() {
            refs.len()
        } else {
            rows.iter().filter(|r| matches!(r, Row::Ref { .. })).count()
        };
        Section {
            refs,
            rows,
            matches,
        }
    });
    Trees { key, sections }
}

/// A section of the column: caret, icon, title and on the right what `header_right` adds (a "+");
/// the body only when open. A click on the header opens or closes it.
pub(super) fn section(
    ui: &mut Ui,
    salt: &str,
    icon: &str,
    title: &str,
    default_open: bool,
    header_right: impl FnOnce(&mut Ui),
    body: impl FnOnce(&mut Ui),
) {
    let id = ui.make_persistent_id(salt);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW + 4.0), Sense::click());
    if response.clicked() {
        state.toggle(ui);
    }
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let caret = if state.is_open() {
        icons::CARET_DOWN
    } else {
        icons::CARET_RIGHT
    };
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            // Labels that are not selectable: a click on the caret or the icon opens the section.
            ui.style_mut().interaction.selectable_labels = false;
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_space(2.0);
            ui.label(icons::text(caret, 11.0).weak());
            ui.label(icons::text(icon, 14.0));
            // The button on the right first: the title takes the rest, cut if needed.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                header_right(ui);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(title).strong())
                            .truncate()
                            .selectable(false),
                    );
                });
            });
        },
    );
    state.show_body_unindented(ui, body);
}

/// The "+" of a section header.
pub(super) fn add_button(ui: &mut Ui, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(icons::text(icons::PLUS, 13.0)).frame(false),
    )
}

/// The indices of the `n` rows of height [`ROW`] starting at `top` that fall in `clip`.
pub(super) fn visible_rows(n: usize, top: f32, clip: egui::Rangef) -> std::ops::Range<usize> {
    let first = ((clip.min - top) / ROW).floor().max(0.0) as usize;
    let last = ((clip.max - top) / ROW).ceil().max(0.0) as usize;
    first.min(n)..last.min(n)
}

/// `n` rows of height [`ROW`], drawn by `row` only if in view: the others are empty space.
pub(super) fn virtual_rows(ui: &mut Ui, n: usize, mut row: impl FnMut(&mut Ui, usize)) {
    let width = ui.available_width();
    let (block, _) = ui.allocate_exact_size(vec2(width, n as f32 * ROW), Sense::hover());
    for i in visible_rows(n, block.top(), ui.clip_rect().y_range()) {
        let rect = Rect::from_min_size(block.min + vec2(0.0, i as f32 * ROW), vec2(width, ROW));
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("row", i))
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| row(ui, i),
        );
    }
}

/// What every row of the column needs.
struct Column<'a> {
    repo: &'a RepoView,
    /// The full name of the current branch.
    current: Option<&'a str>,
    can_write: bool,
    merge: merge::Gate,
    filtering: bool,
}

impl Column<'_> {
    /// The rows of a section; the folders clicked end up in `toggled` (`<salt>:<path>`).
    fn rows(
        &self,
        ui: &mut Ui,
        salt: &str,
        section: &Section,
        toggled: &mut Vec<String>,
        actions: &mut Vec<Action>,
    ) {
        if section.rows.is_empty() {
            ui.weak(tr!("refs-none"));
            return;
        }
        virtual_rows(ui, section.rows.len(), |ui, i| match &section.rows[i] {
            Row::Folder {
                depth,
                path,
                name,
                count,
                open,
            } => {
                let remote = (salt == "ref-remote" && *depth == 0)
                    .then(|| self.repo.remotes.list().unwrap_or_default())
                    .and_then(|list| list.iter().find(|r| &r.name == name));
                let clicked = match remote {
                    Some(remote) => remote_row(ui, remote, *open, *count, self.can_write, actions),
                    None => folder_row(ui, *depth, path, name, *count, *open),
                };
                if clicked && !self.filtering {
                    toggled.push(format!("{salt}:{path}"));
                }
            }
            Row::Ref { depth, index, leaf } => {
                let info = &self.repo.refs[section.refs[*index]];
                self.ref_row(ui, info, *depth, *leaf, actions);
            }
        });
    }

    /// A ref: its icon (a check on the current branch), the last part of the name, ahead/behind.
    /// Click: shows its commit; double click on a local branch: checkout.
    fn ref_row(
        &self,
        ui: &mut Ui,
        info: &RefInfo,
        depth: usize,
        leaf: usize,
        actions: &mut Vec<Action>,
    ) {
        let is_head = self.current == Some(info.name.as_str());
        // The id comes from the ref name, not from the position: if the refs are reread between
        // the two clicks, a double click does not land on another branch.
        let response = row_response(ui, ui.make_persistent_id(&info.name));
        indent(ui, depth);
        ui.add_space(CARET);
        let icon = match (info.kind, is_head) {
            (_, true) => icons::text(icons::CHECK, 13.0).color(ui.visuals().hyperlink_color),
            (RefKind::Tag, _) => icons::text(icons::TAG, 13.0).weak(),
            _ => icons::text(icons::BRANCH, 13.0).weak(),
        };
        ui.label(icon);
        let name = RichText::new(&info.short_name()[leaf..]);
        let name = if is_head { name.strong() } else { name };
        ui.add(egui::Label::new(name).truncate().selectable(false));
        if let Some(upstream) = &info.upstream {
            upstream_label(ui, upstream);
        }
        let local = info.kind == RefKind::LocalBranch;
        if response.double_clicked() && local && !is_head && self.can_write {
            actions.push(Action::Branch(branch::Action::Checkout(
                info.short_name().to_owned(),
            )));
        } else if response.clicked() {
            actions.push(Action::SelectCommit(info.target));
        }
        let response = response.on_hover_text(ref_tooltip(info, is_head));
        response.context_menu(|ui| {
            let branch = matches!(info.kind, RefKind::LocalBranch | RefKind::RemoteBranch);
            if branch && !is_head {
                let current = self
                    .current
                    .map_or("HEAD", |c| c.strip_prefix(HEADS_PREFIX).unwrap_or(c));
                compare_item(ui, info, current, actions);
            }
            let on_remote = (info.kind == RefKind::RemoteBranch)
                .then(|| {
                    remotes::split_remote_branch(info.short_name(), &self.repo.remotes.names())
                })
                .flatten();
            let remotes = if info.kind == RefKind::Tag {
                self.repo.remotes.names()
            } else {
                Vec::new()
            };
            ref_menu(
                ui,
                info,
                is_head,
                on_remote,
                &remotes,
                self.can_write,
                &self.merge,
                actions,
            );
        });
    }
}

/// "Remotes": each remote with its branches under it, "+" to add one.
fn remotes_section(
    ui: &mut Ui,
    column: &Column<'_>,
    branches: &Section,
    toggled: &mut Vec<String>,
    actions: &mut Vec<Action>,
) {
    let repo = column.repo;
    let list = repo.remotes.list();
    if column.filtering && branches.matches == 0 {
        return;
    }
    let title = match list {
        Some(list) => tr!("remotes-section-count", n = list.len()),
        None => tr!("remotes-section"),
    };
    let add = |ui: &mut Ui| {
        let button = add_button(ui, column.can_write)
            .on_hover_text(tr!("remotes-add-button-hint"))
            .on_disabled_hover_text(wait());
        if button.clicked() {
            actions.push(Action::Remotes(remotes::Action::Add));
        }
    };
    let mut body_actions = Vec::new();
    section(ui, "ref-remote", icons::CLOUD, &title, true, add, |ui| {
        match list {
            Some([]) if branches.rows.is_empty() => {
                ui.weak(tr!("remotes-none"));
            }
            None if repo.remotes.list_error().is_none() => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak(tr!("remotes-loading"));
                });
            }
            _ => {}
        }
        if let Some(error) = repo.remotes.list_error() {
            ui.add(
                egui::Label::new(
                    RichText::new(tr!("remotes-list-error", error = error))
                        .color(ui.visuals().error_fg_color)
                        .small(),
                )
                .wrap(),
            );
        }
        if !branches.rows.is_empty() {
            column.rows(ui, "ref-remote", branches, toggled, &mut body_actions);
        }
    });
    actions.extend(body_actions);
}

/// The click area of a whole row, lit under the mouse. The labels drawn after it in the row are not
/// selectable, so a click on the icon or the name reaches the row.
pub(super) fn row_response(ui: &mut Ui, id: egui::Id) -> egui::Response {
    ui.style_mut().interaction.selectable_labels = false;
    let rect = ui.max_rect();
    let response = ui.interact(rect, id, Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    response
}

fn indent(ui: &mut Ui, depth: usize) {
    ui.add_space(6.0 + depth as f32 * INDENT);
}

/// The caret of an open or closed folder.
pub(super) fn caret(ui: &mut Ui, open: bool) {
    let icon = if open {
        icons::CARET_DOWN
    } else {
        icons::CARET_RIGHT
    };
    ui.add_sized(
        vec2(CARET, ROW),
        egui::Label::new(icons::text(icon, 10.0).weak()).selectable(false),
    );
}

/// A folder (`feature` of `feature/login`): caret, icon, name and how many refs it holds. Returns
/// whether it was clicked (to open or close it).
fn folder_row(ui: &mut Ui, depth: usize, path: &str, name: &str, count: usize, open: bool) -> bool {
    let response = row_response(ui, ui.make_persistent_id(("folder", path)));
    indent(ui, depth);
    caret(ui, open);
    ui.label(icons::text(icons::FOLDER, 13.0).weak());
    ui.add(egui::Label::new(name).truncate().selectable(false));
    ui.label(RichText::new(count.to_string()).small().weak());
    response.clicked()
}

pub(super) fn ref_tooltip(info: &RefInfo, is_head: bool) -> String {
    let (name, target) = (&info.name, info.target.to_string());
    match info.kind {
        RefKind::LocalBranch if is_head => {
            tr!("refs-tooltip-head", name = name, target = &target)
        }
        RefKind::LocalBranch => tr!("refs-tooltip-local", name = name, target = &target),
        RefKind::RemoteBranch | RefKind::Tag => {
            tr!("refs-tooltip-other", name = name, target = &target)
        }
        RefKind::Stash => tr!("refs-tooltip-stash", name = name, target = &target),
    }
}

/// The context menu of a ref: on local branches checkout, new branch, new tag, merge into the current
/// branch, rename, pull, push and delete; on remote branches and tags new branch, new tag and merge, and on
/// remote branches "Delete from the remote…" (`on_remote`: the remote and the branch on it); on tags
/// push and delete, also from each of the configured `remotes`.
#[allow(clippy::too_many_arguments)]
pub(super) fn ref_menu(
    ui: &mut Ui,
    info: &RefInfo,
    is_head: bool,
    on_remote: Option<(String, String)>,
    remotes: &[String],
    can_write: bool,
    merge: &merge::Gate,
    actions: &mut Vec<Action>,
) {
    let name = info.short_name();
    if info.kind == RefKind::LocalBranch {
        let button = ui
            .add_enabled(
                can_write && !is_head,
                egui::Button::new(tr!("refs-checkout")),
            )
            .on_hover_text(tr!("refs-checkout-hint", name = name))
            .on_disabled_hover_text(if is_head {
                tr!("refs-already-active")
            } else {
                wait()
            });
        if button.clicked() {
            actions.push(Action::Branch(branch::Action::Checkout(name.to_owned())));
            ui.close();
        }
    }
    let button = ui
        .add_enabled(can_write, egui::Button::new(tr!("refs-new-branch")))
        .on_hover_text(tr!("refs-new-branch-hint", id = info.target.short()))
        .on_disabled_hover_text(wait());
    if button.clicked() {
        let (start, proposed) = Start::from_ref(info);
        actions.push(Action::Branch(branch::Action::NewBranch(start, proposed)));
        ui.close();
    }
    let button = ui
        .add_enabled(can_write, egui::Button::new(tr!("refs-new-tag")))
        .on_hover_text(tr!("refs-new-tag-hint", id = info.target.short()))
        .on_disabled_hover_text(wait());
    if button.clicked() {
        actions.push(Action::Branch(branch::Action::NewTag(
            Start::from_ref(info).0,
        )));
        ui.close();
    }
    if !is_head {
        merge_items(ui, name, name, merge, can_write, actions);
    }
    if let Some((remote, branch)) = on_remote {
        ui.separator();
        let color = if can_write {
            ui.visuals().error_fg_color
        } else {
            ui.visuals().weak_text_color()
        };
        let button = ui
            .add_enabled(
                can_write,
                egui::Button::new(RichText::new(tr!("refs-delete-remote")).color(color)),
            )
            .on_hover_text(tr!(
                "refs-delete-remote-hint",
                branch = &branch,
                remote = &remote
            ))
            .on_disabled_hover_text(wait());
        if button.clicked() {
            actions.push(Action::Remotes(remotes::Action::DeleteBranch(
                remote, branch,
            )));
            ui.close();
        }
    }
    if info.kind == RefKind::Tag {
        tag_items(ui, name, remotes, can_write, actions);
    }
    if info.kind != RefKind::LocalBranch {
        return;
    }
    if !is_head {
        ui.separator();
    }
    let button = ui
        .add_enabled(can_write, egui::Button::new(tr!("refs-rename")))
        .on_disabled_hover_text(wait());
    if button.clicked() {
        actions.push(Action::Branch(branch::Action::RenameBranch(
            name.to_owned(),
        )));
        ui.close();
    }
    ui.separator();
    let pull_hint = match (&info.upstream, is_head) {
        (None, _) => tr!("refs-pull-no-upstream", name = name),
        (Some(up), true) => tr!("refs-pull-head-hint", upstream = remote_short(&up.name)),
        (Some(up), false) => tr!(
            "refs-pull-other-hint",
            upstream = remote_short(&up.name),
            name = name
        ),
    };
    // One name everywhere: the tooltip says whether it is a fast-forward only.
    let button = ui
        .add_enabled(
            can_write && info.upstream.is_some(),
            egui::Button::new(tr!("refs-pull")),
        )
        .on_hover_text(&pull_hint)
        .on_disabled_hover_text(if info.upstream.is_some() {
            wait()
        } else {
            pull_hint
        });
    if button.clicked() {
        actions.push(Action::Sync(sync::Action::Pull(
            Some(name.to_owned()),
            None,
        )));
        ui.close();
    }
    let button = ui
        .add_enabled(can_write, egui::Button::new(tr!("refs-push")))
        .on_hover_text(match &info.upstream {
            Some(up) => tr!(
                "refs-push-hint",
                name = name,
                upstream = remote_short(&up.name)
            ),
            None => tr!("refs-push-first-hint", name = name),
        })
        .on_disabled_hover_text(wait());
    if button.clicked() {
        actions.push(Action::Sync(sync::Action::Push(Some(name.to_owned()))));
        ui.close();
    }
    ui.separator();
    let enabled = can_write && !is_head;
    let color = if enabled {
        ui.visuals().error_fg_color
    } else {
        ui.visuals().weak_text_color()
    };
    let button = ui
        .add_enabled(
            enabled,
            egui::Button::new(RichText::new(tr!("refs-delete")).color(color)),
        )
        .on_hover_text(tr!("refs-delete-hint"))
        .on_disabled_hover_text(if is_head {
            tr!("refs-delete-head")
        } else {
            wait()
        });
    if button.clicked() {
        actions.push(Action::Branch(branch::Action::DeleteBranch(
            name.to_owned(),
            info.target,
        )));
        ui.close();
    }
}

/// The entries of a tag: "Push to “`<remote>`”" for each remote, "Delete…" (only here) and
/// "Delete from “`<remote>`”…" for each remote. All of them ask the engine, which confirms the deletions.
fn tag_items(
    ui: &mut Ui,
    name: &str,
    remotes: &[String],
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    ui.separator();
    if remotes.is_empty() {
        ui.add_enabled(false, egui::Button::new(tr!("refs-tag-push")))
            .on_disabled_hover_text(tr!("refs-tag-no-remote"));
    }
    for remote in remotes {
        let button = ui
            .add_enabled(
                can_write,
                egui::Button::new(tr!("refs-tag-push-to", remote = remote)),
            )
            .on_hover_text(tr!("refs-tag-push-hint", name = name, remote = remote))
            .on_disabled_hover_text(wait());
        if button.clicked() {
            actions.push(Action::Sync(sync::Action::PushTag(
                remote.clone(),
                name.to_owned(),
            )));
            ui.close();
        }
    }
    ui.separator();
    let color = if can_write {
        ui.visuals().error_fg_color
    } else {
        ui.visuals().weak_text_color()
    };
    let button = ui
        .add_enabled(
            can_write,
            egui::Button::new(RichText::new(tr!("refs-tag-delete")).color(color)),
        )
        .on_hover_text(tr!("refs-tag-delete-hint", name = name))
        .on_disabled_hover_text(wait());
    if button.clicked() {
        actions.push(Action::Branch(branch::Action::DeleteTag(name.to_owned())));
        ui.close();
    }
    for remote in remotes {
        let button = ui
            .add_enabled(
                can_write,
                egui::Button::new(
                    RichText::new(tr!("refs-tag-delete-remote", remote = remote)).color(color),
                ),
            )
            .on_hover_text(tr!(
                "refs-tag-delete-remote-hint",
                name = name,
                remote = remote
            ))
            .on_disabled_hover_text(wait());
        if button.clicked() {
            actions.push(Action::Remotes(remotes::Action::DeleteTag(
                remote.clone(),
                name.to_owned(),
            )));
            ui.close();
        }
    }
}

/// "Compare with `<current branch>`": what the branch brings since it diverged from HEAD.
/// `current` is the short name of the current branch, or `HEAD` when detached.
fn compare_item(ui: &mut Ui, info: &RefInfo, current: &str, actions: &mut Vec<Action>) {
    let name = info.short_name();
    let button = ui
        .button(tr!("compare-with-current", current = current))
        .on_hover_text(tr!(
            "compare-with-current-hint",
            name = name,
            current = current
        ));
    if button.clicked() {
        actions.push(Action::Compare(crate::compare::Action::Branch {
            rev: info.name.clone(),
            name: name.to_owned(),
        }));
        ui.close();
    }
    ui.separator();
}

/// The "Merge into `<branch>`" entries (fast-forward if enough) and "... with merge commit" (`no_ff`) for
/// `rev`, shown as `what`; disabled with the reason if the current branch does not allow it.
pub(super) fn merge_items(
    ui: &mut Ui,
    rev: &str,
    what: &str,
    gate: &merge::Gate,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    ui.separator();
    let active = tr!("refs-current-branch");
    let (into, blocked) = match gate {
        merge::Gate::Into(branch) => (branch.as_str(), None),
        merge::Gate::Blocked(reason) => (active.as_str(), Some(reason.as_str())),
    };
    let items = [
        (
            tr!("refs-merge-item", into = into),
            false,
            tr!("refs-merge-hint", what = what, into = into),
        ),
        (
            tr!("refs-merge-no-ff-item", into = into),
            true,
            tr!("refs-merge-no-ff-hint", what = what, into = into),
        ),
    ];
    for (label, no_ff, hint) in items {
        let button = ui
            .add_enabled(can_write && blocked.is_none(), egui::Button::new(label))
            .on_hover_text(hint)
            .on_disabled_hover_text(blocked.map_or_else(wait, str::to_owned));

        if button.clicked() {
            actions.push(Action::Merge(merge::Action::Merge {
                rev: rev.to_owned(),
                what: what.to_owned(),
                no_ff,
            }));
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_rows_in_view_are_drawn() {
        let clip = |min: f32, max: f32| egui::Rangef::new(min, max);
        // 100 rows from y = 50: the view 50..150 holds the first five.
        assert_eq!(visible_rows(100, 50.0, clip(50.0, 150.0)), 0..5);
        // Scrolled: from row 10, a partial row at the bottom included.
        assert_eq!(visible_rows(100, 50.0, clip(250.0, 355.0)), 10..16);
        // Past the end, or above the start.
        assert_eq!(visible_rows(3, 50.0, clip(0.0, 1000.0)), 0..3);
        assert_eq!(visible_rows(100, 500.0, clip(0.0, 100.0)), 0..0);
    }
}
