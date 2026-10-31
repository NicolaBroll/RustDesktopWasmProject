use super::*;
use crate::{clean, commit, conflict, diff, discard, stage};

/// Stage area, in place of the details when the changes row is selected: the lists
/// "Staged" and "Unstaged", each virtualized, and below the commit message box.
/// `writing` is true while a write is in progress: the actions that modify the repo
/// are disabled. `draft` is the message draft, which the text fields change.
pub(crate) fn stage_panel(
    ui: &mut Ui,
    repo: &RepoView,
    draft: &mut CommitDraft,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    // The commit box takes what the part above leaves, measured in the previous frame: headers
    // and one row per list stay visible, and in a low window the box's body loses rows.
    let layout_id = ui.make_persistent_id("stage-layout");
    let mut layout: StageLayout = ui.data(|d| d.get_temp(layout_id)).unwrap_or_default();
    let room = ui.available_height() - layout.upper_min - layout.box_frame - layout.box_fixed;
    let row = ui.text_style_height(&egui::TextStyle::Body);
    let body_rows = ((room - BODY_MARGIN) / row)
        .floor()
        .clamp(1.0, BODY_ROWS as f32) as usize;
    let panel = egui::Panel::bottom("commit-message")
        .resizable(false)
        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 6)))
        .show(ui, |ui| {
            let top = ui.cursor().top();
            let body = commit_box(ui, repo, draft, writing, body_rows, actions);
            (ui.cursor().top() - top, body)
        });
    let (content, body) = panel.inner;
    layout.box_fixed = content - body;
    layout.box_frame = panel.response.rect.height() - content;
    let top = ui.cursor().top();
    let mut lists = 0.0;
    ui.add_space(4.0);
    let stash = tr!("stage-stash-button");
    title_row(
        ui,
        RichText::new(tr!("stage-heading")).heading().into(),
        &[&stash],
        0.0,
        |ui| {
            stash_push_button(
                ui,
                repo,
                !writing && !repo.modal_open(),
                egui::Button::new(stash.as_str()),
                actions,
            );
        },
    );
    if writing {
        ui.horizontal(|ui| {
            ui.spinner();
            let label = repo
                .write_label
                .clone()
                .unwrap_or_else(|| tr!("stage-writing-fallback"));
            ui.weak(tr!("stage-in-progress", label = label));
        });
    }
    if let Some(error) = &repo.write_error {
        ui.horizontal_top(|ui| {
            if ui
                .small_button("x")
                .on_hover_text(tr!("stage-hide"))
                .clicked()
            {
                actions.push(Action::Stage(stage::Action::DismissError));
            }
            ui.add(
                egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap(),
            );
        });
    }
    ui.separator();
    let enabled = !writing && !repo.modal_open();
    let open = repo.diff.target();
    // The two lists share the space; a short list leaves the rest to the other.
    let section_header = 28.0;
    let spare = (ui.available_height() - 2.0 * section_header - 12.0).max(80.0);
    let staged_h = list_height(repo.stage.staged.len())
        .min((spare / 2.0).max(spare - list_height(repo.stage.unstaged.len())));
    // Under the staged list: the other section, its header perhaps on two rows.
    let sides = [
        (
            Side::Staged,
            &repo.stage.staged,
            staged_h,
            2.0 * section_header + 40.0,
        ),
        (Side::Unstaged, &repo.stage.unstaged, f32::INFINITY, 0.0),
    ];
    for (i, (side, items, height, below)) in sides.into_iter().enumerate() {
        if i > 0 {
            ui.separator();
        }
        let space = Space { height, below };
        let shown = stage_section(ui, side, items, open, enabled, space, actions);
        // A list needs one row; beyond that it can give its room to the commit box.
        let least = if items.is_empty() {
            0.0
        } else {
            FILE_ROW_HEIGHT
        };
        lists += shown - least;
    }
    layout.upper_min = ui.cursor().top() - top - lists;
    let before: Option<StageLayout> = ui.data(|d| d.get_temp(layout_id));
    if before.is_none_or(|before| before.differs(&layout)) {
        ui.data_mut(|d| d.insert_temp(layout_id, layout));
        // Measured anew: this frame is laid out again with the new heights.
        ui.ctx().request_discard("stage panel layout");
        ui.ctx().request_repaint();
    }
}

/// The heights the stage panel measures in a frame for the next one.
#[derive(Clone, Copy, Default)]
struct StageLayout {
    /// The part above the commit box with one row per list.
    upper_min: f32,
    /// The commit box without its body field.
    box_fixed: f32,
    /// The panel of the commit box around its content: margins and separator.
    box_frame: f32,
}

impl StageLayout {
    /// More than half a point apart: below that a new pass would change nothing.
    fn differs(&self, other: &Self) -> bool {
        [
            self.upper_min - other.upper_min,
            self.box_fixed - other.box_fixed,
            self.box_frame - other.box_frame,
        ]
        .iter()
        .any(|d| d.abs() > 0.5)
    }
}

/// The vertical margin of a text field around its rows (egui's default).
const BODY_MARGIN: f32 = 4.0;

/// Rows of the message body, when the window has room for them.
pub(super) const BODY_ROWS: usize = 4;

/// Maximum height of git's output after a failed commit; beyond that it scrolls.
pub(super) const OUTPUT_HEIGHT: f32 = 140.0;

/// Commit message box: subject with the counter, body with `body_rows` rows, amend, "Commit"
/// button (also Ctrl+Enter) and the error of the last commit. Returns the height of the body
/// field.
pub(super) fn commit_box(
    ui: &mut Ui,
    repo: &RepoView,
    draft: &mut CommitDraft,
    writing: bool,
    body_rows: usize,
    actions: &mut Vec<Action>,
) -> f32 {
    let sending = repo.commit.sending().is_some();
    let gate = Gate {
        draft,
        ..repo.commit_gate(writing)
    }
    .check();
    // Before the text fields, which would otherwise receive the key.
    let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Enter);
    if !repo.modal_open() && ui.input_mut(|i| i.consume_shortcut(&shortcut)) && gate.is_ok() {
        actions.push(Action::Commit(commit::Action::Commit));
    }

    ui.horizontal(|ui| {
        ui.strong(tr!("stage-commit-message"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let len = draft.subject_len();
            let text = format!("{len}/{}", commit::SUBJECT_LIMIT);
            if draft.subject_too_long() {
                ui.label(
                    RichText::new(text)
                        .small()
                        .color(ui.visuals().warn_fg_color),
                )
                .on_hover_text(tr!("stage-subject-too-long", limit = commit::SUBJECT_LIMIT));
            } else {
                ui.label(RichText::new(text).small().weak())
                    .on_hover_text(tr!("stage-subject-count-hint"));
            }
        });
    });
    ui.add_enabled(
        !sending,
        egui::TextEdit::singleline(&mut draft.subject)
            .id_salt("commit-subject")
            .hint_text(tr!("stage-subject-hint"))
            .desired_width(f32::INFINITY),
    );
    let body = ui.add_enabled(
        !sending,
        egui::TextEdit::multiline(&mut draft.body)
            .id_salt("commit-body")
            .hint_text(tr!("stage-body-hint"))
            .desired_rows(body_rows)
            .desired_width(f32::INFINITY),
    );
    let body_height = body.rect.height();

    let mut amend = draft.amend();
    let toggle = ui
        .add_enabled(
            !sending && !writing,
            egui::Checkbox::new(&mut amend, tr!("stage-amend-checkbox")),
        )
        .on_hover_text(tr!("stage-amend-help"));
    if toggle.changed() {
        actions.push(Action::Commit(commit::Action::SetAmend(amend)));
    }
    if draft.amend() {
        let mut text = match repo.head_commit() {
            Some(c) => tr!(
                "stage-amend-warning-summary",
                id = c.id.short(),
                summary = &c.summary
            ),
            None => match repo.head.target() {
                Some(id) => tr!("stage-amend-warning-id", id = id.short()),
                None => tr!("stage-amend-warning"),
            },
        };
        if draft.is_proposed() {
            text.push(' ');
            text.push_str(&tr!("stage-amend-proposed"));
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
            tr!("stage-commit-amend-button")
        } else {
            tr!("stage-commit-button")
        };
        // The primary action: the accent fill when it can go, egui's grey when it cannot.
        let button = if gate.is_ok() {
            let (fill, text) = theme::primary(ui.visuals());
            egui::Button::new(RichText::new(label).color(text)).fill(fill)
        } else {
            egui::Button::new(label)
        };
        let button = ui.add_enabled(gate.is_ok(), button);
        let button = match gate {
            Ok(()) => button.on_hover_text(tr!("stage-commit-help")),
            Err(blocked) => button.on_disabled_hover_text(blocked.hint()),
        };
        if button.clicked() {
            actions.push(Action::Commit(commit::Action::Commit));
        }
        if sending {
            ui.spinner();
            let label = repo
                .write_label
                .clone()
                .unwrap_or_else(|| tr!("stage-commit-button"));
            ui.weak(tr!("stage-in-progress", label = label));
        } else {
            ui.weak(tr!("stage-commit-shortcut"));
        }
    });
    // Why the button is disabled, without having to hover, on one row cut to the width (the
    // whole text is in the button's tooltip). During work and when the commit can go the row
    // stays empty: the box does not move while typing, after a commit or when a dialog opens.
    let hint = match gate {
        Err(commit::Blocked::Busy) | Ok(()) => " ".to_owned(),
        Err(blocked) => blocked.hint(),
    };
    ui.add(egui::Label::new(RichText::new(hint).small().weak()).truncate());

    if let Some(error) = repo.commit.error() {
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            if ui
                .small_button("x")
                .on_hover_text(tr!("stage-hide"))
                .clicked()
            {
                actions.push(Action::Commit(commit::Action::DismissError));
            }
            ui.add(
                egui::Label::new(RichText::new(&error.message).color(ui.visuals().error_fg_color))
                    .wrap(),
            );
        });
        if let Some(output) = &error.output {
            egui::ScrollArea::vertical()
                .id_salt("commit-output")
                .max_height(OUTPUT_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    // A `&str` makes the field read-only, but selectable and copyable.
                    ui.add(
                        egui::TextEdit::multiline(&mut output.as_str())
                            .id_salt("commit-output-text")
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(1)
                            .desired_width(f32::INFINITY),
                    );
                });
            if ui
                .small_button(tr!("stage-copy-output"))
                .on_hover_text(tr!("stage-copy-output-help"))
                .clicked()
            {
                ui.ctx().copy_text(output.clone());
            }
        }
    }
    body_height
}

/// The title on the left and the buttons (`labels`, plus `extra` points of icon buttons, added
/// by `add_buttons` from the right) on one row when the title fits whole; otherwise the title
/// on its own row and the buttons under it, so neither is cut.
fn title_row(
    ui: &mut Ui,
    title: egui::WidgetText,
    labels: &[&str],
    extra: f32,
    add_buttons: impl FnOnce(&mut Ui),
) {
    let width = |text: egui::WidgetText| {
        text.into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
        .size()
        .x
    };
    let spacing = ui.spacing().item_spacing.x;
    let padding = ui.spacing().button_padding.x;
    let buttons: f32 = labels
        .iter()
        .map(|label| width((*label).into()) + 2.0 * padding + spacing)
        .sum::<f32>()
        + extra;
    let fits = width(title.clone()) + spacing + buttons <= ui.available_width();
    if fits {
        egui::Sides::new().show(ui, |ui| ui.label(title), add_buttons);
    } else {
        ui.add(egui::Label::new(title).wrap());
        egui::Sides::new().show(ui, |_| {}, add_buttons);
    }
}

/// The room of a section's list.
#[derive(Clone, Copy)]
pub(super) struct Space {
    /// The most the list may take.
    pub height: f32,
    /// What to leave free under it.
    pub below: f32,
}

/// Height needed by a list of `rows` files, at least one row.
pub(super) fn list_height(rows: usize) -> f32 {
    rows.max(1) as f32 * (FILE_ROW_HEIGHT + 2.0)
}

/// Title with the count, button for the whole list (and for the unstaged one the "⋯" menu with
/// "Discard all…"), then the files. Returns the height given to the list, 0 when it is empty.
pub(super) fn stage_section(
    ui: &mut Ui,
    side: Side,
    items: &[StageItem],
    open: Option<&DiffTarget>,
    enabled: bool,
    space: Space,
    actions: &mut Vec<Action>,
) -> f32 {
    let (title, op, all_label, all_help) = match side {
        Side::Staged => (
            tr!("stage-staged-title"),
            StageOp::Unstage,
            tr!("stage-unstage-all"),
            tr!("stage-unstage-all-help"),
        ),
        Side::Unstaged => (
            tr!("stage-unstaged-title"),
            StageOp::Stage,
            tr!("stage-stage-all"),
            tr!("stage-stage-all-help"),
        ),
    };
    // The "⋯" icon button: the icon and the button's padding.
    let more_width = match side {
        Side::Staged => 0.0,
        Side::Unstaged => {
            MORE_ICON + 2.0 * ui.spacing().button_padding.x + ui.spacing().item_spacing.x
        }
    };
    let heading = tr!("stage-section-title", title = &title, n = items.len());
    let title = RichText::new(heading).strong().into();
    title_row(ui, title, &[&all_label], more_width, |ui| {
        // Right to left: the menu at the edge, "Stage all" before it, apart from the
        // destructive entry.
        if side == Side::Unstaged {
            unstaged_menu(ui, items, enabled, actions);
        }
        let button = ui
            .add_enabled(
                enabled && !items.is_empty(),
                egui::Button::new(all_label.as_str()).small(),
            )
            .on_hover_text(all_help);
        if button.clicked()
            && let Some(paths) = stage::all_paths(items)
        {
            actions.push(Action::Stage(stage::Action::Write(op, paths)));
        }
    });
    if items.is_empty() {
        let text = match side {
            Side::Staged => tr!("stage-empty-staged"),
            Side::Unstaged => tr!("stage-empty-unstaged"),
        };
        ui.weak(text);
        return 0.0;
    }
    // After the header, whose height depends on the width; at least a row, but never over the
    // commit box under it: in a low window the list scrolls.
    let available = ui.available_height();
    let height = space
        .height
        .min(available - space.below)
        .max(FILE_ROW_HEIGHT)
        .min(available)
        .max(0.0);
    let source = match side {
        Side::Staged => DiffSource::Index,
        Side::Unstaged => DiffSource::Worktree,
    };
    let font = theme::body();
    let mono = theme::mono();
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    let button_label = match op {
        StageOp::Stage => tr!("stage-stage"),
        StageOp::Unstage => tr!("stage-unstage"),
    };
    let output = egui::ScrollArea::vertical()
        .id_salt(("stage-area", side == Side::Staged))
        .auto_shrink([false, true])
        .max_height(height)
        // egui keeps at least 64 px of a scrolled area: here the room computed above wins.
        .min_scrolled_height(height)
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
                let selected = open == Some(&target);
                // Over the button the row's response is not hovered: the pointer decides.
                let hovered = ui.rect_contains_pointer(rect);
                let (text, weak) = if selected {
                    let color = ui.visuals().selection.stroke.color;
                    (color, color.gamma_multiply(0.75))
                } else {
                    (text, weak)
                };
                if selected {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                } else if hovered {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let painter = ui.painter_at(rect);
                let badge = egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 11.0, rect.center().y),
                    vec2(18.0, 16.0),
                );
                painter.rect_filled(badge, 3.0, theme::change(item.change));
                painter.text(
                    badge.center(),
                    Align2::CENTER_CENTER,
                    change_letter(item.change),
                    mono.clone(),
                    theme::ON_BADGE,
                );

                // On the right the row's button, only under the mouse and on the open file: a
                // column of buttons on every row is noise.
                let mut clicked = false;
                let mut right = rect.right() - 4.0;
                if hovered || selected {
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
                    clicked = cell
                        .add_enabled(enabled, egui::Button::new(button_label.as_str()).small())
                        .on_hover_text(row_action_help(op, item))
                        .clicked();
                    right = button_rect.left() - 4.0;
                }

                let left = badge.right() + 6.0;
                let job = stage_path_job(item, &font, text, weak, right - left);
                let galley = painter.layout_job(job);
                let top = rect.center().y - galley.size().y / 2.0;
                painter.galley(egui::pos2(left, top), galley, text);

                let response = response.on_hover_text(stage_tooltip(op, item));
                if (clicked || response.double_clicked()) && enabled {
                    actions.push(Action::Stage(stage::Action::Write(
                        op,
                        vec![item.path.clone()],
                    )));
                } else if response.clicked() {
                    actions.push(if item.is_conflict() && op == StageOp::Stage {
                        Action::Conflict(conflict::Action::Open(item.path.clone()))
                    } else {
                        Action::Diff(diff::Action::Open(target.clone()))
                    });
                }
                response.context_menu(|ui| {
                    file_menu(ui, op, item, &target, enabled, actions);
                });
            }
        });
    output.inner_rect.height()
}

/// Size of the "⋯" icon of the section menu.
const MORE_ICON: f32 = 14.0;

/// The "⋯" menu of the unstaged list: "Discard all…" and, with untracked files, "Delete all
/// untracked files…", in the destructive red as in the file
/// context menu, kept away from "Stage all" so a misclick does not open its dialog.
fn unstaged_menu(ui: &mut Ui, items: &[StageItem], enabled: bool, actions: &mut Vec<Action>) {
    let more = egui::Button::new(icons::text(icons::MORE, MORE_ICON)).small();
    egui::containers::menu::MenuButton::from_button(more)
        .ui(ui, |ui| {
            let paths = discard::discardable_paths(items);
            let can = enabled && paths.is_some();
            let color = if can {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().weak_text_color()
            };
            let label = RichText::new(tr!("stage-discard-all")).color(color);
            let button = ui
                .add_enabled(can, egui::Button::new(label))
                .on_hover_text(tr!("stage-discard-all-help"));
            if button.clicked()
                && let Some(paths) = paths
            {
                actions.push(Action::Discard(discard::Action::Discard(paths)));
                ui.close();
            }
            // Only when there are untracked files: deleting is not discarding.
            if let Some(paths) = clean::untracked_paths(items) {
                let color = if enabled {
                    ui.visuals().error_fg_color
                } else {
                    ui.visuals().weak_text_color()
                };
                let label =
                    RichText::new(tr!("stage-delete-untracked", n = paths.len())).color(color);
                let button = ui
                    .add_enabled(enabled, egui::Button::new(label))
                    .on_hover_text(tr!("stage-delete-untracked-help"));
                if button.clicked() {
                    actions.push(Action::Clean(clean::Action::Delete(paths)));
                    ui.close();
                }
            }
        })
        .0
        .on_hover_text(tr!("stage-more-hint"));
}

/// The context menu of a file of the stage area: diff, stage/unstage and, when unstaged,
/// "Discard changes…".
pub(super) fn file_menu(
    ui: &mut Ui,
    op: StageOp,
    item: &StageItem,
    target: &DiffTarget,
    enabled: bool,
    actions: &mut Vec<Action>,
) {
    if item.is_conflict()
        && op == StageOp::Stage
        && ui.button(tr!("stage-resolve-in-editor")).clicked()
    {
        actions.push(Action::Conflict(conflict::Action::Open(item.path.clone())));
        ui.close();
    }
    if ui.button(tr!("stage-show-diff")).clicked() {
        actions.push(Action::Diff(diff::Action::Open(target.clone())));
        ui.close();
    }
    let blame_off = match item.change {
        Change::Untracked => Some(tr!("stage-blame-untracked")),
        Change::Deleted => Some(tr!("stage-blame-deleted")),
        _ => None,
    };
    file_menu_entries(ui, &item.path, None, blame_off, actions);
    ui.separator();
    let label = match op {
        StageOp::Stage if item.is_conflict() => tr!("stage-mark-resolved"),
        StageOp::Stage => tr!("stage-stage"),
        StageOp::Unstage => tr!("stage-unstage"),
    };
    let button = ui
        .add_enabled(enabled, egui::Button::new(label))
        .on_hover_text(row_action_help(op, item));
    if button.clicked() {
        actions.push(Action::Stage(stage::Action::Write(
            op,
            vec![item.path.clone()],
        )));
        ui.close();
    }
    if op == StageOp::Stage && clean::deletable(item) {
        ui.separator();
        let color = if enabled {
            ui.visuals().error_fg_color
        } else {
            ui.visuals().weak_text_color()
        };
        let button = ui
            .add_enabled(
                enabled,
                egui::Button::new(RichText::new(tr!("stage-delete-file")).color(color)),
            )
            .on_hover_text(tr!("stage-delete-file-help"))
            .on_disabled_hover_text(tr!("stage-wait"));
        if button.clicked() {
            actions.push(Action::Clean(clean::Action::Delete(vec![
                item.path.clone(),
            ])));
            ui.close();
        }
    } else if op == StageOp::Stage {
        ui.separator();
        let reason = discard::not_discardable_reason(item);
        let button = ui
            .add_enabled(
                enabled && reason.is_none(),
                egui::Button::new(RichText::new(tr!("stage-discard-file")).color(
                    if reason.is_none() && enabled {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().weak_text_color()
                    },
                )),
            )
            .on_hover_text(tr!("stage-discard-file-help"))
            .on_disabled_hover_text(reason.unwrap_or_else(|| tr!("stage-wait")));
        if button.clicked() {
            actions.push(Action::Discard(discard::Action::Discard(vec![
                item.path.clone(),
            ])));
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

pub(super) fn conflict_name(conflict: Conflict) -> String {
    match conflict {
        Conflict::BothDeleted => tr!("stage-conflict-both-deleted"),
        Conflict::AddedByUs => tr!("stage-conflict-added-by-us"),
        Conflict::DeletedByThem => tr!("stage-conflict-deleted-by-them"),
        Conflict::AddedByThem => tr!("stage-conflict-added-by-them"),
        Conflict::DeletedByUs => tr!("stage-conflict-deleted-by-us"),
        Conflict::BothAdded => tr!("stage-conflict-both-added"),
        Conflict::BothModified => tr!("stage-conflict-both-modified"),
    }
}

pub(super) fn row_action_help(op: StageOp, item: &StageItem) -> String {
    match op {
        StageOp::Stage if item.is_conflict() => tr!("stage-row-mark-resolved-help"),
        StageOp::Stage => tr!("stage-row-stage-help"),
        StageOp::Unstage => tr!("stage-row-unstage-help"),
    }
}

pub(super) fn stage_tooltip(op: StageOp, item: &StageItem) -> String {
    let mut lines = Vec::new();
    let path = match &item.original {
        Some(original) => format!("{original} -> {}", item.path),
        None => item.path.to_string(),
    };
    match item.conflict {
        Some(conflict) => lines.push(tr!(
            "stage-tooltip-conflict",
            path = &path,
            conflict = conflict_name(conflict)
        )),
        None => lines.push(format!("{path} ({})", format::change_name(item.change))),
    }
    lines.push(tr!("stage-tooltip-click"));
    lines.push(match op {
        StageOp::Stage if item.is_conflict() => tr!("stage-tooltip-double-resolve"),
        StageOp::Stage => tr!("stage-tooltip-double-stage"),
        StageOp::Unstage => tr!("stage-tooltip-double-unstage"),
    });
    if op == StageOp::Stage && discard::discardable(item) {
        lines.push(tr!("stage-tooltip-right-discard"));
    } else if op == StageOp::Stage && clean::deletable(item) {
        lines.push(tr!("stage-tooltip-right-delete"));
    } else {
        lines.push(tr!("stage-tooltip-right"));
    }
    lines.join("\n")
}
