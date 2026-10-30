//! Blame and history of a file (S34), in place of the graph. State in [`crate::file`].

use super::*;
use crate::diff::{Pane, display_line};
use crate::diff_view::{self, LinesUi};
use crate::file::{self, BlameDoc, FileView, Loaded, Mode};
use egui::{Rect, pos2};
use project_domain::{BlameCommit, FileLogEntry, RepoPath};

/// Height of a blame row: all equal, for `show_rows`.
const ROW_HEIGHT: f32 = 18.0;
/// Height of a history row.
const ENTRY_HEIGHT: f32 = 38.0;
const PAD: f32 = 6.0;
/// Width of the commit column of the blame.
const INFO_W: f32 = 360.0;

/// "Blame", "File history" and "Copy path" in the context menu of a file. `rev`: the commit (blame at it,
/// history from it), `None` for the working tree (history from HEAD). `blame_off`: why the
/// blame cannot be asked for (e.g. a deleted file).
pub(super) fn file_menu_entries(
    ui: &mut Ui,
    path: &RepoPath,
    rev: Option<Oid>,
    blame_off: Option<String>,
    actions: &mut Vec<Action>,
) {
    let open = |mode| {
        Action::File(file::Action::Open {
            path: path.clone(),
            rev,
            mode,
        })
    };
    let blame = ui
        .add_enabled(
            blame_off.is_none(),
            egui::Button::new(tr!("file-menu-blame")),
        )
        .on_hover_text(match rev {
            Some(id) => tr!("file-menu-blame-hint-commit", id = id.short()),
            None => tr!("file-menu-blame-hint-worktree"),
        })
        .on_disabled_hover_text(blame_off.unwrap_or_default());
    if blame.clicked() {
        actions.push(open(Mode::Blame));
        ui.close();
    }
    let history = ui
        .button(tr!("file-menu-history"))
        .on_hover_text(match rev {
            Some(id) => tr!("file-menu-history-hint-commit", id = id.short()),
            None => tr!("file-menu-history-hint-head"),
        });
    if history.clicked() {
        actions.push(open(Mode::History));
        ui.close();
    }
    if ui
        .button(tr!("file-menu-copy-path"))
        .on_hover_text(tr!("file-menu-copy-path-hint"))
        .clicked()
    {
        ui.ctx().copy_text(path.to_string());
        ui.close();
    }
}

/// The view: bar with the file and the tabs, then the blame or the history.
pub(crate) fn file_view(
    ui: &mut Ui,
    view: &FileView,
    pane: Pane<'_>,
    can_write: bool,
    split: bool,
    actions: &mut Vec<Action>,
) {
    toolbar(ui, view, actions);
    ui.separator();
    match view.mode {
        Mode::Blame => blame(ui, view, actions),
        Mode::History => history(ui, view, pane, can_write, split, actions),
    }
}

fn toolbar(ui: &mut Ui, view: &FileView, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        if ui
            .button(tr!("file-close"))
            .on_hover_text(tr!("file-close-hint"))
            .clicked()
        {
            actions.push(Action::File(file::Action::Close));
        }
        ui.separator();
        for (mode, label) in [
            (Mode::Blame, tr!("file-tab-blame")),
            (Mode::History, tr!("file-tab-history")),
        ] {
            if ui.selectable_label(view.mode == mode, label).clicked() && view.mode != mode {
                actions.push(Action::File(file::Action::Mode(mode)));
            }
        }
        ui.separator();
        ui.strong(view.path.to_string_lossy());
        let at = match (view.mode, view.rev) {
            (Mode::Blame, Some(id)) => tr!("file-at-commit", id = id.short()),
            (Mode::Blame, None) => tr!("file-at-worktree"),
            (Mode::History, Some(id)) => tr!("file-from-commit", id = id.short()),
            (Mode::History, None) => tr!("file-from-head"),
        };
        let label = ui.weak(at);
        if let Some(id) = view.rev {
            label.on_hover_text(id.to_string());
        }
        if view.mode == Mode::History {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut follow = view.follow;
                if ui
                    .checkbox(&mut follow, tr!("file-follow"))
                    .on_hover_text(tr!("file-follow-hint"))
                    .changed()
                {
                    actions.push(Action::File(file::Action::Follow(follow)));
                }
            });
        }
    });
}

/// Spinner with a text, or the error with "Retry".
fn loading(ui: &mut Ui, text: String) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.weak(text);
    });
}

fn failed(ui: &mut Ui, error: &str, actions: &mut Vec<Action>) {
    ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap());
    if ui.button(tr!("file-retry")).clicked() {
        actions.push(Action::File(file::Action::Retry));
    }
}

fn blame(ui: &mut Ui, view: &FileView, actions: &mut Vec<Action>) {
    match view.blame() {
        Loaded::Loading => loading(ui, tr!("file-blame-loading")),
        Loaded::Failed(error) => failed(ui, error, actions),
        Loaded::Ready(doc) if doc.blame.lines.is_empty() => {
            ui.centered_and_justified(|ui| ui.weak(tr!("file-blame-empty")));
        }
        Loaded::Ready(doc) => blame_rows(ui, doc, actions),
    }
}

/// Colors and measures read once per frame.
struct Style {
    mono: FontId,
    small: FontId,
    char_w: f32,
    text: Color32,
    weak: Color32,
    accent: Color32,
    gutter_bg: Color32,
    alt_bg: Color32,
    hover_bg: Color32,
    /// Commit column plus line numbers.
    gutter_w: f32,
}

fn blame_rows(ui: &mut Ui, doc: &BlameDoc, actions: &mut Vec<Action>) {
    let mono = theme::mono();
    let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&mono, '0'));
    let numbers_w = (doc.number_width as f32) * char_w + 2.0 * PAD;
    let style = Style {
        small: theme::small(),
        char_w,
        text: ui.visuals().text_color(),
        weak: ui.visuals().weak_text_color(),
        accent: if ui.visuals().dark_mode {
            Color32::from_rgb(0xe3, 0xb3, 0x41)
        } else {
            Color32::from_rgb(0x9a, 0x67, 0x00)
        },
        gutter_bg: ui.visuals().panel_fill,
        alt_bg: ui.visuals().faint_bg_color,
        hover_bg: ui.visuals().widgets.hovered.weak_bg_fill,
        gutter_w: INFO_W + numbers_w,
        mono,
    };
    let content_w = style.gutter_w + PAD + (doc.max_cols + 4) as f32 * style.char_w;
    let commits = &doc.blame.commits;
    egui::ScrollArea::both()
        .id_salt("blame")
        .auto_shrink(false)
        .show_rows(ui, ROW_HEIGHT, doc.blame.lines.len(), |ui, range| {
            let width = content_w.max(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for index in range.clone() {
                let line = &doc.blame.lines[index];
                let run_index = doc.run_at(index);
                let run = doc.runs[run_index];
                let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
                let painter = ui.painter();
                // The gutter stays visible on the left even when scrolling horizontally.
                let left = rect.left().max(ui.clip_rect().left());
                let gutter =
                    Rect::from_min_size(pos2(left, rect.top()), vec2(style.gutter_w, ROW_HEIGHT));
                let info = Rect::from_min_size(gutter.left_top(), vec2(INFO_W, ROW_HEIGHT));
                let commit = run.commit.and_then(|c| commits.get(c));
                let response = ui.interact(
                    info,
                    ui.scope_id().with(("blame-gutter", index)),
                    if commit.is_some() {
                        Sense::click()
                    } else {
                        Sense::hover()
                    },
                );
                if run_index % 2 == 1 {
                    painter.rect_filled(rect, 0.0, style.alt_bg);
                }
                let mid = rect.center().y;
                paint_line(ui, &line.text.to_string_lossy(), &style, rect, mid);
                // Opaque gutter above the text scrolling under it.
                painter.rect_filled(gutter, 0.0, style.gutter_bg);
                if run_index % 2 == 1 {
                    painter.rect_filled(gutter, 0.0, style.alt_bg);
                }
                if response.hovered() && commit.is_some() {
                    painter.rect_filled(info, 0.0, style.hover_bg);
                }
                // The commit on the first line of the run (and on the first visible one).
                if index == run.start || index == range.start {
                    let job = info_job(commit, &style, INFO_W - 2.0 * PAD);
                    let galley = painter.layout_job(job);
                    painter.galley(
                        pos2(info.left() + PAD, mid - galley.size().y / 2.0),
                        galley,
                        style.text,
                    );
                }
                painter.text(
                    pos2(gutter.right() - PAD, mid),
                    Align2::RIGHT_CENTER,
                    (index + 1).to_string(),
                    style.mono.clone(),
                    style.weak,
                );
                match commit {
                    Some(commit) => {
                        let response = response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(commit_tooltip(commit));
                        if response.clicked() {
                            actions.push(Action::SelectCommit(commit.id));
                        }
                    }
                    None => {
                        response.on_hover_text(tr!("file-not-committed-hint"));
                    }
                }
            }
        });
}

/// Short id, date, author and summary on one line, cut with `…`.
fn info_job(commit: Option<&BlameCommit>, style: &Style, width: f32) -> egui::text::LayoutJob {
    let format = |font: &FontId, color| egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    match commit {
        Some(c) => {
            job.append(&c.id.short(), 0.0, format(&style.mono, style.accent));
            let date = date_time(&c.author);
            let date = date.split(' ').next().unwrap_or_default();
            job.append(date, 2.0 * PAD, format(&style.small, style.weak));
            job.append(&c.author.name, 2.0 * PAD, format(&style.small, style.weak));
            job.append(&c.summary, 2.0 * PAD, format(&style.small, style.text));
        }
        None => job.append(
            &tr!("file-not-committed"),
            0.0,
            format(&style.small, style.accent),
        ),
    }
    job.wrap.max_width = width.max(10.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    job
}

fn commit_tooltip(c: &BlameCommit) -> String {
    let mut lines = vec![
        c.id.to_string(),
        c.summary.clone(),
        tr!(
            "file-tooltip-author",
            author = signer(&c.author),
            date = date_time_zone(&c.author)
        ),
    ];
    if c.committer.name != c.author.name || c.committer.email != c.author.email {
        lines.push(tr!(
            "file-tooltip-committer",
            committer = signer(&c.committer),
            date = date_time_zone(&c.committer)
        ));
    }
    if c.boundary {
        lines.push(tr!("file-tooltip-boundary"));
    }
    lines.push(tr!("file-tooltip-select"));
    lines.join("\n")
}

/// The text of a line, with expanded tabs and dim control characters.
fn paint_line(ui: &Ui, content: &str, style: &Style, rect: Rect, mid: f32) {
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
    let galley = ui.painter().layout_job(job);
    let x = rect.left() + style.gutter_w + PAD;
    ui.painter()
        .galley(pos2(x, mid - galley.size().y / 2.0), galley, style.text);
}

/// The history on the left, the diff of the selected entry on the right.
fn history(
    ui: &mut Ui,
    view: &FileView,
    pane: Pane<'_>,
    can_write: bool,
    split: bool,
    actions: &mut Vec<Action>,
) {
    let open = match pane {
        Pane::Closed => None,
        Pane::Loading(t) | Pane::Loaded(t, _) | Pane::Failed(t, _) => Some(t),
    };
    let max = (ui.available_width() * 0.7).max(240.0);
    egui::Panel::left("file-log-list")
        .resizable(true)
        .default_size((ui.available_width() * 0.45).clamp(300.0, 560.0))
        .min_size(240.0)
        .max_size(max)
        .show(ui, |ui| history_list(ui, view, open, actions));
    match pane {
        Pane::Closed => {
            ui.centered_and_justified(|ui| ui.weak(tr!("file-history-pick")));
        }
        pane => {
            let lines = LinesUi {
                selection: None,
                can_write,
                back: false,
                split,
            };
            diff_view::diff_panel(ui, pane, lines, actions);
        }
    }
}

fn history_list(
    ui: &mut Ui,
    view: &FileView,
    open: Option<&DiffTarget>,
    actions: &mut Vec<Action>,
) {
    let history = &view.history;
    if view.log_first_loading() {
        loading(ui, tr!("file-history-loading"));
        return;
    }
    if history.entries.is_empty() {
        match &history.error {
            Some(error) => failed(ui, error, actions),
            None => {
                ui.weak(tr!("file-history-empty"));
            }
        }
        return;
    }
    let tail = history.has_more || history.error.is_some() || view.log_loading();
    let count = history.entries.len() + usize::from(tail);
    let font = theme::body();
    let mono = theme::mono();
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    egui::ScrollArea::vertical()
        .id_salt("file-history-list")
        .auto_shrink(false)
        .show_rows(ui, ENTRY_HEIGHT, count, |ui, range| {
            for index in range {
                let Some(entry) = history.entries.get(index) else {
                    tail_row(ui, view, actions);
                    continue;
                };
                let (rect, response) = ui
                    .allocate_exact_size(vec2(ui.available_width(), ENTRY_HEIGHT), Sense::click());
                let selected = open.is_some_and(|t| view.entry_target(index).as_ref() == Some(t));
                if selected {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                } else if response.hovered() {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let painter = ui.painter_at(rect);
                let badge = Rect::from_center_size(
                    pos2(rect.left() + 11.0, rect.top() + 11.0),
                    vec2(18.0, 16.0),
                );
                painter.rect_filled(badge, 3.0, theme::change(entry.status));
                painter.text(
                    badge.center(),
                    Align2::CENTER_CENTER,
                    change_letter(entry.status),
                    mono.clone(),
                    theme::ON_BADGE,
                );
                let left = badge.right() + 6.0;
                let width = rect.right() - left - 4.0;
                let (top, bottom) =
                    entry_jobs(entry, &view.path, &font, &mono, [text, weak], width);
                let top = painter.layout_job(top);
                painter.galley(
                    pos2(left, rect.top() + 11.0 - top.size().y / 2.0),
                    top,
                    text,
                );
                let bottom = painter.layout_job(bottom);
                let y = rect.bottom() - 10.0 - bottom.size().y / 2.0;
                painter.galley(pos2(left, y), bottom, text);
                let response = response.on_hover_text(entry_tooltip(entry));
                if response.clicked() {
                    actions.push(Action::File(file::Action::Select(index)));
                }
                response.context_menu(|ui| {
                    if ui.button(tr!("file-select-commit")).clicked() {
                        actions.push(Action::SelectCommit(entry.commit.id));
                        ui.close();
                    }
                });
            }
        });
}

/// The last row: "Load more", the page on its way or its error.
fn tail_row(ui: &mut Ui, view: &FileView, actions: &mut Vec<Action>) {
    ui.allocate_ui(vec2(ui.available_width(), ENTRY_HEIGHT), |ui| {
        ui.horizontal_centered(|ui| {
            if view.log_loading() {
                ui.spinner();
                ui.weak(tr!("file-history-loading"));
            } else if let Some(error) = &view.history.error {
                ui.colored_label(ui.visuals().error_fg_color, tr!("file-history-page-error"))
                    .on_hover_text(error);
                if ui.small_button(tr!("file-retry")).clicked() {
                    actions.push(Action::File(file::Action::Retry));
                }
            } else if ui.small_button(tr!("file-load-more")).clicked() {
                actions.push(Action::File(file::Action::More));
            }
        });
    });
}

/// Two lines: the summary (with the path and the rename, if any), then id, date and author.
fn entry_jobs(
    entry: &FileLogEntry,
    current: &RepoPath,
    font: &FontId,
    mono: &FontId,
    [text, weak]: [Color32; 2],
    width: f32,
) -> (egui::text::LayoutJob, egui::text::LayoutJob) {
    let format = |font: &FontId, color| egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let one_line = |mut job: egui::text::LayoutJob| {
        job.wrap.max_width = width.max(10.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        job
    };
    let c = &entry.commit;
    let mut top = egui::text::LayoutJob::default();
    top.append(&c.summary, 0.0, format(font, text));
    let notes: Vec<String> = [file::entry_path(entry, current), file::entry_origin(entry)]
        .into_iter()
        .flatten()
        .collect();
    if !notes.is_empty() {
        top.append(&format!("({})", notes.join(", ")), PAD, format(font, weak));
    }
    let small = theme::small();
    let mut bottom = egui::text::LayoutJob::default();
    bottom.append(&c.id.short(), 0.0, format(mono, weak));
    bottom.append(&date_time(&c.author), PAD, format(&small, weak));
    bottom.append(&c.author.name, PAD, format(&small, weak));
    (one_line(top), one_line(bottom))
}

fn entry_tooltip(entry: &FileLogEntry) -> String {
    let c = &entry.commit;
    let mut lines = vec![
        c.id.to_string(),
        c.summary.clone(),
        tr!(
            "file-tooltip-author",
            author = signer(&c.author),
            date = date_time_zone(&c.author)
        ),
        format!(
            "{} ({})",
            entry.path.to_string_lossy(),
            format::change_name(entry.status)
        ),
    ];
    if let Some(origin) = file::entry_origin(entry) {
        lines.push(origin);
    }
    lines.push(tr!("file-history-click"));
    lines.join("\n")
}
