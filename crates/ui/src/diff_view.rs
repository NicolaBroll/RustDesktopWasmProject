//! Drawing of the diff view, in place of the graph (`impl-docs/ARCHITECTURE-git.md` §8).
//! Reads the state of [`crate::diff`] and produces [`Action`]; changes nothing directly.

use crate::diff::{
    self, DiffDoc, DiffSource, DiffTarget, Pane, Row, SplitRow, display_line, file_kind,
    file_summary, file_title, line_number, line_prefix,
};
use crate::icons;
use crate::lines::{self, LineButtons, LineSelection, Partial};
use crate::settings::{self, Change};
use crate::{Action, Drag};
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, pos2, vec2};
use project_domain::LineKind;
use project_i18n::tr;

/// Height of a diff row: all equal, for `show_rows`.
const ROW_HEIGHT: f32 = 18.0;
/// Margin before the text and between columns.
const PAD: f32 = 6.0;

/// What the view needs for the row selection.
#[derive(Clone, Copy)]
pub(crate) struct LinesUi<'a> {
    /// The selected rows of the open diff.
    pub selection: Option<&'a LineSelection>,
    /// False during a write or with a confirmation open.
    pub can_write: bool,
    /// Shows "Back" (closes the diff): not in the file history, where the list stays beside it.
    pub back: bool,
    /// Side by side (`true`) or unified: a setting, the toolbar changes it.
    pub split: bool,
}

/// The partial selection for the loaded diff: whether it is possible, and the buttons.
struct LineState<'a> {
    partial: Partial,
    selection: &'a LineSelection,
    buttons: Option<LineButtons>,
}

pub(crate) fn diff_panel(
    ui: &mut Ui,
    pane: Pane<'_>,
    lines_ui: LinesUi<'_>,
    actions: &mut Vec<Action>,
) {
    let target = match pane {
        Pane::Closed => return,
        Pane::Loading(t) | Pane::Loaded(t, _) | Pane::Failed(t, _) => t,
    };
    let empty = LineSelection::default();
    let state = match pane {
        Pane::Loaded(target, doc) => {
            let partial = lines::partial(doc, target.source);
            let selection = lines_ui.selection.unwrap_or(&empty);
            let buttons = lines::buttons(&partial, selection, lines_ui.can_write);
            Some(LineState {
                partial,
                selection,
                buttons,
            })
        }
        _ => None,
    };
    let split = lines_ui.split;
    toolbar(ui, target, pane, lines_ui.back, split, actions);
    if let Some(state) = &state {
        if split {
            // The rows are selected in the unified view: here the hunks only.
            if state.buttons.is_some() {
                ui.add(
                    egui::Label::new(egui::RichText::new(tr!("diff-split-lines-hint")).weak())
                        .wrap(),
                );
            }
        } else {
            lines_bar(ui, state, actions);
        }
    }
    ui.separator();
    match pane {
        Pane::Closed => {}
        Pane::Loading(target) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(tr!(
                    "diff-loading",
                    path = target.path.to_string_lossy().as_ref()
                ));
            });
        }
        Pane::Failed(_, error) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button(tr!("diff-retry")).clicked() {
                actions.push(Action::Diff(diff::Action::Retry));
            }
        }
        Pane::Loaded(target, doc) if doc.diff.files.is_empty() => {
            let text = match target.source {
                DiffSource::Worktree => tr!("diff-empty-worktree"),
                _ => tr!("diff-empty"),
            };
            ui.centered_and_justified(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(text).weak()).wrap())
            });
        }
        Pane::Loaded(_, doc) => {
            if let Some(state) = &state {
                if split {
                    split_view(ui, doc, state.buttons, actions);
                } else {
                    rows(ui, doc, state, actions);
                }
            }
        }
    }
}

/// Close, path and commit, rows added and removed. The counters and the view toggles keep
/// their place on the right; path and description are cut before them.
fn toolbar(
    ui: &mut Ui,
    target: &DiffTarget,
    pane: Pane<'_>,
    back: bool,
    split: bool,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        if back {
            if ui
                .button(tr!("diff-back"))
                .on_hover_text(tr!("diff-back-hint"))
                .clicked()
            {
                actions.push(Action::Diff(diff::Action::Close));
            }
            ui.separator();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let layouts = [
                (true, icons::SPLIT, tr!("diff-split-hint")),
                (false, icons::UNIFIED, tr!("diff-unified-hint")),
            ];
            for (value, icon, hint) in layouts {
                let button = egui::Button::selectable(split == value, icons::text(icon, 14.0));
                if ui.add(button).on_hover_text(hint).clicked() && split != value {
                    actions.push(Action::Settings(settings::Action::Change(
                        Change::DiffSplit(value),
                    )));
                }
            }
            if let Pane::Loaded(_, doc) = pane
                && doc.line_counts() != (0, 0)
            {
                let (added, removed) = doc.line_counts();
                ui.separator();
                ui.colored_label(removed_text(ui), format!("-{removed}"));
                ui.colored_label(added_text(ui), format!("+{added}"));
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let (text, hint) = match target.source {
                    DiffSource::Commit(id) => {
                        (tr!("diff-in-commit", id = id.short()), id.to_string())
                    }
                    DiffSource::Worktree => (tr!("diff-unstaged"), tr!("diff-unstaged-hint")),
                    DiffSource::Index => (tr!("diff-staged"), tr!("diff-staged-hint")),
                };
                let path = target.path.to_string_lossy();
                ui.add(egui::Label::new(egui::RichText::new(path.as_ref()).strong()).truncate())
                    .on_hover_text(format!("{path} {text}"));
                // The description whole or not at all: a lone "…" says nothing.
                let description = egui::WidgetText::from(egui::RichText::new(text).weak())
                    .into_galley(
                        ui,
                        Some(egui::TextWrapMode::Extend),
                        f32::INFINITY,
                        egui::TextStyle::Body,
                    );
                if description.size().x <= ui.available_width() {
                    ui.label(description).on_hover_text(hint);
                }
            });
        });
    });
}

/// Under the diff bar: the buttons for the selected rows, or why the file cannot be
/// split into rows.
fn lines_bar(ui: &mut Ui, state: &LineState<'_>, actions: &mut Vec<Action>) {
    if let Partial::Unavailable(reason) = &state.partial {
        ui.horizontal(|ui| {
            ui.weak(tr!("diff-partial-unavailable"))
                .on_hover_text(reason);
            ui.add(egui::Label::new(egui::RichText::new(format!("({reason})")).weak()).truncate())
                .on_hover_text(reason);
        });
        return;
    }
    let Some(b) = state.buttons else {
        return;
    };
    ui.horizontal(|ui| {
        let (label, help) = match b.direction {
            project_domain::PatchDirection::Stage => (
                tr!("diff-stage-selected", n = b.count),
                tr!("diff-stage-selected-hint"),
            ),
            project_domain::PatchDirection::Unstage => (
                tr!("diff-unstage-selected", n = b.count),
                tr!("diff-unstage-selected-hint"),
            ),
        };
        let button = ui
            .add_enabled(b.selected, egui::Button::new(label))
            .on_hover_text(help)
            .on_disabled_hover_text(if b.count == 0 {
                tr!("diff-no-lines-selected")
            } else {
                tr!("diff-wait-write")
            });
        if button.clicked() {
            actions.push(Action::Lines(lines::Action::Selected));
        }
        let clear = ui
            .add_enabled(b.clear, egui::Button::new(tr!("diff-clear")))
            .on_hover_text(tr!("diff-clear-hint"));
        if clear.clicked() {
            actions.push(Action::Lines(lines::Action::Clear));
        }
    });
}

fn added_text(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0x56, 0xd3, 0x64)
    } else {
        Color32::from_rgb(0x1a, 0x7f, 0x37)
    }
}

fn removed_text(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(0xf8, 0x51, 0x49)
    } else {
        Color32::from_rgb(0xcf, 0x22, 0x2e)
    }
}

/// Colors and measures read once per frame.
struct Style {
    mono: FontId,
    title: FontId,
    char_w: f32,
    text: Color32,
    weak: Color32,
    strong: Color32,
    added_bg: Color32,
    removed_bg: Color32,
    added_gutter: Color32,
    removed_gutter: Color32,
    /// Behind the changed words of a paired line.
    added_word: Color32,
    removed_word: Color32,
    hunk_bg: Color32,
    hunk_text: Color32,
    file_bg: Color32,
    gutter_bg: Color32,
    /// Tint of the selected rows and color of the bar on the left.
    selected_bg: Color32,
    selected_bar: Color32,
    hover_bg: Color32,
    /// Width of the number column (two numbers and the sign).
    gutter_w: f32,
    number_w: usize,
}

impl Style {
    fn new(ui: &Ui, doc: &DiffDoc) -> Self {
        let mono = crate::theme::mono();
        let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&mono, '0'));
        let dark = ui.visuals().dark_mode;
        let alpha = |r, g, b, a| Color32::from_rgba_unmultiplied(r, g, b, a);
        let (added_bg, removed_bg, added_gutter, removed_gutter) = if dark {
            (
                alpha(0x2e, 0xa0, 0x43, 46),
                alpha(0xf8, 0x51, 0x49, 46),
                alpha(0x2e, 0xa0, 0x43, 90),
                alpha(0xf8, 0x51, 0x49, 90),
            )
        } else {
            (
                alpha(0x2d, 0xa4, 0x4e, 40),
                alpha(0xf0, 0x40, 0x40, 36),
                alpha(0x2d, 0xa4, 0x4e, 80),
                alpha(0xf0, 0x40, 0x40, 72),
            )
        };
        let number_w = doc.number_width;
        // `old new +`, with margins.
        let gutter_w = (2 * number_w + 2) as f32 * char_w + 4.0 * PAD;
        Self {
            mono,
            title: crate::theme::body(),
            char_w,
            text: ui.visuals().text_color(),
            weak: ui.visuals().weak_text_color(),
            strong: ui.visuals().strong_text_color(),
            added_bg,
            removed_bg,
            added_gutter,
            removed_gutter,
            added_word: alpha(0x2e, 0xa0, 0x43, if dark { 110 } else { 90 }),
            removed_word: alpha(0xf8, 0x51, 0x49, if dark { 110 } else { 80 }),
            hunk_bg: alpha(0x4c, 0x9a, 0xf0, if dark { 34 } else { 28 }),
            hunk_text: if dark {
                Color32::from_rgb(0x8c, 0xb8, 0xf0)
            } else {
                Color32::from_rgb(0x2a, 0x5d, 0xa8)
            },
            file_bg: ui.visuals().faint_bg_color,
            gutter_bg: ui.visuals().panel_fill,
            selected_bg: {
                let c = ui.visuals().selection.bg_fill;
                alpha(c.r(), c.g(), c.b(), if dark { 110 } else { 90 })
            },
            selected_bar: if dark {
                Color32::from_rgb(0x58, 0xa6, 0xff)
            } else {
                Color32::from_rgb(0x09, 0x69, 0xda)
            },
            hover_bg: ui
                .visuals()
                .widgets
                .hovered
                .weak_bg_fill
                .gamma_multiply(0.5),
            gutter_w,
            number_w,
        }
    }
}

/// The diff rows, virtualized: only the visible ones are drawn, even with thousands of
/// rows. The text scrolls horizontally; the line numbers stay fixed on the left.
fn rows(ui: &mut Ui, doc: &DiffDoc, state: &LineState<'_>, actions: &mut Vec<Action>) {
    let style = Style::new(ui, doc);
    let buttons = state.buttons;
    let selectable_lines = buttons.is_some();
    let (pointer, down) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.primary_down()));
    if state.selection.is_dragging() && !down {
        actions.push(Action::Lines(lines::Action::Drag(Drag::End)));
    }
    // Width of the content: the widest row, plus the cut sign.
    let text_w = (doc.max_cols + 28) as f32 * style.char_w;
    let content_w = style.gutter_w + PAD + text_w;
    egui::ScrollArea::both()
        .id_salt("diff")
        .auto_shrink(false)
        .show_rows(ui, ROW_HEIGHT, doc.rows.len(), |ui, range| {
            let width = content_w.max(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for (index, row) in doc.rows[range.clone()].iter().enumerate() {
                let index = range.start + index;
                let selectable = selectable_lines && lines::selectable(doc, index);
                let sense = if selectable {
                    Sense::click_and_drag()
                } else {
                    Sense::hover()
                };
                let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), sense);
                // The numbers stay visible on the left even when scrolling horizontally.
                let gutter_left = rect.left().max(ui.clip_rect().left());
                let gutter = Rect::from_min_size(
                    pos2(gutter_left, rect.top()),
                    vec2(style.gutter_w, ROW_HEIGHT),
                );
                let mark = if state.selection.contains(index) {
                    Mark::Selected
                } else if selectable && response.hovered() {
                    Mark::Hover
                } else {
                    Mark::None
                };
                paint_row(ui, doc, row, rect, gutter, &style, mark);
                if selectable {
                    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if response.clicked() {
                        let shift = ui.input(|i| i.modifiers.shift);
                        actions.push(Action::Lines(lines::Action::Select { row: index, shift }));
                    } else if response.drag_started() {
                        actions.push(Action::Lines(lines::Action::Drag(Drag::Start(index))));
                    } else if state.selection.is_dragging()
                        && down
                        && pointer.is_some_and(|p| rect.contains(p))
                    {
                        actions.push(Action::Lines(lines::Action::Drag(Drag::Over(index))));
                    }
                }
                if let (Row::Hunk(_, h), Some(b)) = (row, buttons) {
                    hunk_button(ui, rect, *h, b, style.hunk_bg, actions);
                }
            }
        });
}

/// How to highlight a changed row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    None,
    Hover,
    Selected,
}

/// "Stage hunk"/"Unstage hunk" on the right of the header, in the visible part, over a strip
/// of `background` that cuts the header text before it.
fn hunk_button(
    ui: &mut Ui,
    rect: Rect,
    hunk: usize,
    b: LineButtons,
    background: Color32,
    actions: &mut Vec<Action>,
) {
    let behind = ui.painter().add(egui::Shape::Noop);
    let right = rect.right().min(ui.clip_rect().right()) - PAD;
    let cell = Rect::from_min_max(pos2(right - 140.0, rect.top()), pos2(right, rect.bottom()));
    let mut cell = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(cell)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    let (label, help) = match b.direction {
        project_domain::PatchDirection::Stage => {
            (tr!("diff-stage-hunk"), tr!("diff-stage-hunk-hint"))
        }
        project_domain::PatchDirection::Unstage => {
            (tr!("diff-unstage-hunk"), tr!("diff-unstage-hunk-hint"))
        }
    };
    let button = cell
        .add_enabled(b.hunks, egui::Button::new(label).small())
        .on_hover_text(help)
        .on_disabled_hover_text(tr!("diff-wait-write"));
    let strip = Rect::from_x_y_ranges(button.rect.left() - 2.0 * PAD..=right + PAD, rect.y_range());
    // The hunk color is translucent: on the panel color, so the text under it is hidden.
    let under = egui::Shape::rect_filled(strip, 0.0, ui.visuals().panel_fill);
    let over = egui::Shape::rect_filled(strip, 0.0, background);
    ui.painter()
        .set(behind, egui::Shape::Vec(vec![under, over]));
    if button.clicked() {
        actions.push(Action::Lines(lines::Action::Hunk(hunk)));
    }
}

fn paint_row(
    ui: &Ui,
    doc: &DiffDoc,
    row: &Row,
    rect: Rect,
    gutter: Rect,
    style: &Style,
    mark: Mark,
) {
    let painter = ui.painter();
    let text_x = rect.left() + style.gutter_w + PAD;
    let mid = rect.center().y;
    match row {
        Row::Gap => {}
        Row::File(f) => {
            let file = &doc.diff.files[*f];
            painter.rect_filled(rect, 0.0, style.file_bg);
            // The title stays visible while scrolling: it starts from the left edge of the view.
            let left = gutter.left() + PAD;
            let title = file_title(file);
            // One file, not renamed: the bar above already shows its path and counters.
            if doc.diff.files.len() == 1 && title == file.path().to_string_lossy() {
                painter.text(
                    pos2(left, mid),
                    Align2::LEFT_CENTER,
                    file_kind(file),
                    style.title.clone(),
                    style.weak,
                );
                return;
            }
            let title = painter.text(
                pos2(left, mid),
                Align2::LEFT_CENTER,
                title,
                style.title.clone(),
                style.strong,
            );
            painter.text(
                pos2(title.right() + 3.0 * PAD, mid),
                Align2::LEFT_CENTER,
                file_summary(file),
                style.title.clone(),
                style.weak,
            );
        }
        Row::Note(_, note) => {
            painter.text(
                pos2(gutter.left() + PAD, mid),
                Align2::LEFT_CENTER,
                note,
                style.title.clone(),
                style.weak,
            );
        }
        Row::Hunk(f, h) => {
            let hunk = &doc.diff.files[*f].hunks[*h];
            painter.rect_filled(rect, 0.0, style.hunk_bg);
            painter.text(
                pos2(text_x, mid),
                Align2::LEFT_CENTER,
                String::from_utf8_lossy(&hunk.header()),
                style.mono.clone(),
                style.hunk_text,
            );
            painter.rect_filled(gutter, 0.0, style.gutter_bg);
            painter.rect_filled(gutter, 0.0, style.hunk_bg);
        }
        Row::NoNewline => {
            painter.text(
                pos2(text_x, mid),
                Align2::LEFT_CENTER,
                "\\ No newline at end of file",
                style.mono.clone(),
                style.weak,
            );
            painter.rect_filled(gutter, 0.0, style.gutter_bg);
        }
        Row::Line(f, h, l) => {
            let line = &doc.diff.files[*f].hunks[*h].lines[*l];
            let (bg, gutter_fill) = match line.kind {
                LineKind::Added => (Some(style.added_bg), Some(style.added_gutter)),
                LineKind::Removed => (Some(style.removed_bg), Some(style.removed_gutter)),
                LineKind::Context => (None, None),
            };
            if let Some(bg) = bg {
                painter.rect_filled(rect, 0.0, bg);
            }
            match mark {
                Mark::Selected => {
                    painter.rect_filled(rect, 0.0, style.selected_bg);
                }
                Mark::Hover => {
                    painter.rect_filled(rect, 0.0, style.hover_bg);
                }
                Mark::None => {}
            }
            let words = match line.kind {
                LineKind::Added => style.added_word,
                _ => style.removed_word,
            };
            paint_words(
                painter,
                doc.emphasis.get(&(*f, *h, *l)),
                text_x,
                rect,
                words,
                style,
            );
            paint_text(
                painter,
                &line.content.to_string_lossy(),
                pos2(text_x, mid),
                style,
            );

            // Number column above the text, opaque: it covers the text scrolling under it.
            painter.rect_filled(gutter, 0.0, style.gutter_bg);
            if let Some(fill) = gutter_fill {
                painter.rect_filled(gutter, 0.0, fill);
            }
            let numbers = format!(
                "{} {} {}",
                line_number(line.old_line, style.number_w),
                line_number(line.new_line, style.number_w),
                line_prefix(line.kind)
            );
            if mark == Mark::Selected {
                painter.rect_filled(gutter, 0.0, style.selected_bg);
                // Full bar on the left and border: the selection is visible even without colors.
                let bar = Rect::from_min_size(gutter.left_top(), vec2(4.0, ROW_HEIGHT));
                painter.rect_filled(bar, 0.0, style.selected_bar);
                painter.rect_stroke(
                    rect.shrink(0.5),
                    0.0,
                    egui::Stroke::new(1.0, style.selected_bar),
                    egui::StrokeKind::Inside,
                );
            }
            painter.text(
                pos2(gutter.left() + PAD, mid),
                Align2::LEFT_CENTER,
                numbers,
                style.mono.clone(),
                if mark == Mark::Selected {
                    style.strong
                } else {
                    style.weak
                },
            );
        }
    }
}

/// The content of a row, with expanded tabs, dim control characters and the cut sign
/// if it is too long.
fn paint_text(painter: &egui::Painter, content: &str, at: egui::Pos2, style: &Style) {
    let shown = display_line(content);
    let format = |color| egui::TextFormat {
        font_id: style.mono.clone(),
        color,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    for (text, special) in &shown.spans {
        job.append(
            text,
            0.0,
            format(if *special { style.weak } else { style.text }),
        );
    }
    if shown.cut > 0 {
        job.append(
            &tr!("diff-line-cut", n = shown.cut),
            0.0,
            format(style.weak),
        );
    }
    let galley = painter.layout_job(job);
    let top = at.y - galley.size().y / 2.0;
    painter.galley(pos2(at.x, top), galley, style.text);
}

/// The background of the changed words (`columns`) of a line whose text starts at `text_x`.
fn paint_words(
    painter: &egui::Painter,
    columns: Option<&Vec<std::ops::Range<usize>>>,
    text_x: f32,
    row: Rect,
    color: Color32,
    style: &Style,
) {
    for range in columns.into_iter().flatten() {
        let word = Rect::from_x_y_ranges(
            text_x + range.start as f32 * style.char_w..=text_x + range.end as f32 * style.char_w,
            row.y_range(),
        );
        painter.rect_filled(word.shrink2(vec2(0.0, 1.0)), 2.0, color);
    }
}

/// The side by side view: the old file on the left, the new one on the right, each line next
/// to the one that replaces it. The text is cut at the middle (no horizontal scroll); the hunks
/// keep their button, the rows are selected in the unified view.
fn split_view(ui: &mut Ui, doc: &DiffDoc, buttons: Option<LineButtons>, actions: &mut Vec<Action>) {
    let style = Style::new(ui, doc);
    egui::ScrollArea::vertical()
        .id_salt("diff-split")
        .auto_shrink(false)
        .show_rows(ui, ROW_HEIGHT, doc.split.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let width = ui.available_width();
            for row in &doc.split[range] {
                let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
                let gutter = Rect::from_min_size(rect.min, vec2(style.gutter_w, ROW_HEIGHT));
                let unified = match row {
                    SplitRow::File(f) => Some(Row::File(*f)),
                    SplitRow::Note(f, note) => Some(Row::Note(*f, note.clone())),
                    SplitRow::Hunk(f, h) => Some(Row::Hunk(*f, *h)),
                    SplitRow::Gap => Some(Row::Gap),
                    SplitRow::Lines(..) => None,
                };
                if let Some(unified) = unified {
                    paint_row(ui, doc, &unified, rect, gutter, &style, Mark::None);
                }
                match row {
                    SplitRow::Hunk(_, h) => {
                        if let Some(b) = buttons {
                            hunk_button(ui, rect, *h, b, style.hunk_bg, actions);
                        }
                    }
                    SplitRow::Lines(f, h, old, new) => {
                        let middle = rect.center().x;
                        let (left, right) = (rect.with_max_x(middle), rect.with_min_x(middle));
                        paint_side(ui, doc, (*f, *h, *old), left, false, &style);
                        paint_side(ui, doc, (*f, *h, *new), right, true, &style);
                        let border = ui.visuals().widgets.noninteractive.bg_stroke;
                        ui.painter().vline(middle, rect.y_range(), border);
                    }
                    _ => {}
                }
            }
        });
}

/// One side of a side by side row: its number, sign and text, or an empty band where this side
/// has no line. `new`: the right side (new line numbers).
fn paint_side(
    ui: &Ui,
    doc: &DiffDoc,
    (f, h, l): (usize, usize, Option<usize>),
    rect: Rect,
    new: bool,
    style: &Style,
) {
    let painter = ui.painter_at(rect);
    let Some(l) = l else {
        painter.rect_filled(rect, 0.0, style.file_bg);
        return;
    };
    let line = &doc.diff.files[f].hunks[h].lines[l];
    let (bg, gutter_fill, words) = match line.kind {
        LineKind::Added => (
            Some(style.added_bg),
            Some(style.added_gutter),
            style.added_word,
        ),
        LineKind::Removed => (
            Some(style.removed_bg),
            Some(style.removed_gutter),
            style.removed_word,
        ),
        LineKind::Context => (None, None, Color32::TRANSPARENT),
    };
    if let Some(bg) = bg {
        painter.rect_filled(rect, 0.0, bg);
    }
    // One number and the sign.
    let gutter_w = (style.number_w + 2) as f32 * style.char_w + 2.0 * PAD;
    let gutter = Rect::from_min_size(rect.min, vec2(gutter_w, ROW_HEIGHT));
    if let Some(fill) = gutter_fill {
        painter.rect_filled(gutter, 0.0, fill);
    }
    let number = if new { line.new_line } else { line.old_line };
    let mid = rect.center().y;
    painter.text(
        pos2(gutter.left() + PAD, mid),
        Align2::LEFT_CENTER,
        format!(
            "{} {}",
            line_number(number, style.number_w),
            line_prefix(line.kind)
        ),
        style.mono.clone(),
        style.weak,
    );
    let text_x = gutter.right() + PAD;
    paint_words(
        &painter,
        doc.emphasis.get(&(f, h, l)),
        text_x,
        rect,
        words,
        style,
    );
    paint_text(
        &painter,
        &line.content.to_string_lossy(),
        pos2(text_x, mid),
        style,
    );
}
