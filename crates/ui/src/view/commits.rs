use super::*;
use crate::settings::DateFormat;
use crate::{branch, compare, merge, rebase, search};

/// Height of a row of the commit list.
pub(super) const ROW_HEIGHT: f32 = 22.0;

/// How the commit list is drawn in this frame.
pub(crate) struct ListOptions {
    /// A write is in progress: the menu entries that write wait.
    pub(crate) writing: bool,
    /// The search field takes the keyboard.
    pub(crate) focus_search: bool,
    pub(crate) dates: DateFormat,
    /// "Check out this commit" asks first (the detached HEAD confirmation is not turned off).
    pub(crate) ask_detach: bool,
}

pub(crate) fn commit_list(
    ui: &mut Ui,
    repo: &RepoView,
    viewport: &mut ListViewport,
    options: ListOptions,
    actions: &mut Vec<Action>,
) {
    let ListOptions {
        writing,
        focus_search,
        dates,
        ask_detach,
    } = options;
    if repo.search.is_open() {
        search_bar(ui, &repo.search, focus_search, actions);
    }
    let can_write = !writing && !repo.modal_open();
    let merge = merge::Gate::new(&repo.head, repo.status.as_ref());
    let menu = CommitMenu {
        repo,
        merge: &merge,
        can_write,
        ask_detach,
    };
    let list = &repo.list;
    // The "Uncommitted changes" row is row 0: commits start at `offset`.
    let header = repo.has_changes();
    let offset = usize::from(header);
    if list.commits().is_empty() && !header {
        let text = if list.is_loading() {
            tr!("commit-list-loading")
        } else {
            tr!("commit-list-empty")
        };
        ui.centered_and_justified(|ui| ui.weak(text));
        return;
    }
    let (style, on_selection) = row_styles(ui, dates);
    // One more row at the bottom while there are more commits to load.
    let rows = offset + list.commits().len() + usize::from(list.has_more());
    let step = ROW_HEIGHT + ui.spacing().item_spacing.y;
    // The graph takes at most two fifths of the list: beyond that it is cut on the right.
    let graph_w = graph::graph_width(list.graph().width).min(ui.available_width() * 0.4);
    let colors = lane_colors(ui, repo);

    let mut area = egui::ScrollArea::vertical().auto_shrink(false);
    let wanted = if repo.scroll_header {
        Some(0)
    } else {
        list.peek_scroll_to().map(|i| i + offset)
    };
    if let Some(target) = scroll_target(wanted, viewport) {
        let top = target as f32 * step;
        // Upward the row goes to the top, downward to the bottom of the view.
        let offset = if target <= viewport.rows.start {
            top
        } else {
            top + step - viewport.height
        };
        area = area.vertical_scroll_offset(offset.max(0.0));
    }

    // The changes row sits in HEAD's column, which is not always the first commit
    // (a freshly fetched remote branch can be more recent).
    let anchor = repo.head.target().and_then(|head| list.anchor(head));
    let output = area.show_rows(ui, ROW_HEIGHT, rows, |ui, range| {
        // Commit rows drawn: the graph goes over the backgrounds of all of them, so it comes after.
        let mut drawn = Vec::with_capacity(range.len());
        let mut header_rect = None;
        for row in range.clone() {
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
            if header && row == 0 {
                let column = anchor.map_or(0, |a| a.column);
                let style = if repo.worktree_selected {
                    &on_selection
                } else {
                    &style
                };
                changes_row(ui, rect, &response, repo, column, graph_w, style);
                header_rect = Some(rect);
                if response.clicked() {
                    actions.push(Action::SelectWorktree);
                }
                continue;
            }
            let index = row - offset;
            let Some(commit) = list.commits().get(index) else {
                let text = match list.seeking() {
                    Some(id) => tr!("commit-list-seeking", id = id.short()),
                    None => tr!("commit-list-loading-more"),
                };
                ui.painter().text(
                    rect.left_center() + vec2(graph_w + 8.0, 0.0),
                    Align2::LEFT_CENTER,
                    text,
                    style.font.clone(),
                    style.weak,
                );
                continue;
            };
            // The two sides of a comparison are both highlighted.
            let selected = (list.selected() == Some(index) && !repo.worktree_selected)
                || repo.compare.marks(commit.id);
            if selected {
                ui.painter()
                    .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            if !selected && repo.search.is_match(commit.id) {
                let tint = theme::search_match(ui.visuals().dark_mode);
                ui.painter().rect_filled(rect, 2.0, tint);
            }
            if response.clicked() {
                // Ctrl+click (Cmd on macOS): compares with the selected commit.
                if ui.input(|i| i.modifiers.command) {
                    actions.push(Action::Compare(compare::Action::With(index)));
                } else {
                    actions.push(Action::Select(index));
                }
            }
            response.context_menu(|ui| menu.show(ui, commit, index, actions));
            let labels = repo.labels.get(&commit.id).map_or(&[][..], Vec::as_slice);
            let text_rect = rect.with_min_x(rect.left() + graph_w);
            let more = commit_row(
                ui,
                text_rect,
                commit,
                labels,
                colors.commit(index),
                if selected { &on_selection } else { &style },
            );
            if let Some(more) = more {
                ui.interact(more, response.id.with("more"), Sense::hover())
                    .on_hover_text(labels_tooltip(labels));
            }
            drawn.push((index, rect));
        }
        if let (Some(rect), Some(anchor)) = (header_rect, anchor) {
            paint_changes_link(ui, rect, anchor, graph_w, step, style.weak);
        }
        paint_graph(ui, list, &colors, &drawn, graph_w, step);
        range
    });
    viewport.rows = output.inner;
    viewport.height = output.inner_rect.height();
    actions.push(Action::Visible(viewport.rows.end.saturating_sub(offset)));
}

/// The style of the rows, and the one of the selected row.
fn row_styles(ui: &Ui, dates: DateFormat) -> (RowStyle, RowStyle) {
    let style = RowStyle {
        font: theme::body(),
        mono: theme::mono(),
        label: theme::small(),
        text: ui.visuals().text_color(),
        weak: ui.visuals().weak_text_color(),
        tag_fill: ui.visuals().widgets.inactive.bg_fill,
        head_fill: ui.visuals().strong_text_color(),
        head_text: ui.visuals().panel_fill,
        now: match dates {
            DateFormat::Absolute => None,
            DateFormat::Relative => Some(unix_now()),
        },
    };
    // On the selected row the text takes the selection's color: gray on blue is hard to read.
    let on_selection = RowStyle {
        text: ui.visuals().selection.stroke.color,
        weak: ui.visuals().selection.stroke.color.gamma_multiply(0.75),
        ..style.clone()
    };
    (style, on_selection)
}

/// The context menu of a commit row.
struct CommitMenu<'a> {
    repo: &'a RepoView,
    merge: &'a merge::Gate,
    /// No write in progress and no dialog open.
    can_write: bool,
    /// "Check out this commit" asks first.
    ask_detach: bool,
}

impl CommitMenu<'_> {
    fn show(&self, ui: &mut Ui, commit: &Commit, index: usize, actions: &mut Vec<Action>) {
        let id = commit.id;
        compare_item(ui, self.repo, index, id, actions);
        if ui
            .button(tr!("commit-copy-id"))
            .on_hover_text(tr!("commit-copy-id-hint", id = id.to_string()))
            .clicked()
        {
            ui.ctx().copy_text(id.to_string());
            ui.close();
        }
        ui.separator();
        let button = ui
            .add_enabled(self.can_write, egui::Button::new(tr!("commit-new-branch")))
            .on_hover_text(tr!("commit-new-branch-hint", id = id.short()))
            .on_disabled_hover_text(wait());
        if button.clicked() {
            actions.push(Action::Branch(branch::Action::NewBranch(
                Start::from_commit(id),
                String::new(),
            )));
            ui.close();
        }
        let button = ui
            .add_enabled(self.can_write, egui::Button::new(tr!("commit-new-tag")))
            .on_hover_text(tr!("commit-new-tag-hint", id = id.short()))
            .on_disabled_hover_text(wait());
        if button.clicked() {
            actions.push(Action::Branch(branch::Action::NewTag(Start::from_commit(
                id,
            ))));
            ui.close();
        }
        checkout_item(ui, self.repo, id, self.can_write, self.ask_detach, actions);
        let what = merge::commit_name(id, &self.repo.refs);
        // Already in HEAD: merging or cherry-picking it would change nothing.
        let in_head = in_head_history(self.repo, id);
        if !in_head {
            merge_items(
                ui,
                &id.to_string(),
                &what,
                self.merge,
                self.can_write,
                actions,
            );
        }
        pick_items(
            ui,
            commit,
            &what,
            self.merge,
            !in_head,
            self.can_write,
            actions,
        );
        rebase_item(
            ui,
            self.repo,
            commit,
            &what,
            self.merge,
            self.can_write,
            actions,
        );
        reset_item(ui, self.repo, commit, &what, self.can_write, actions);
    }
}

/// "Check out this commit" (S55): HEAD detached there, after the confirmation if `ask`. Off
/// when HEAD is already detached at that commit.
fn checkout_item(
    ui: &mut Ui,
    repo: &RepoView,
    id: Oid,
    can_write: bool,
    ask: bool,
    actions: &mut Vec<Action>,
) {
    let here = repo.head == Head::Detached(id);
    let button = ui
        .add_enabled(
            can_write && !here,
            egui::Button::new(tr!("commit-checkout")),
        )
        .on_hover_text(tr!("commit-checkout-hint", id = id.short()));
    let button = if here {
        button.on_disabled_hover_text(tr!("commit-checkout-here"))
    } else {
        button.on_disabled_hover_text(wait())
    };
    if button.clicked() {
        actions.push(Action::Branch(branch::Action::CheckoutCommit { id, ask }));
        ui.close();
    }
}

/// "Compare with the selected commit", when another commit is selected.
fn compare_item(ui: &mut Ui, repo: &RepoView, index: usize, id: Oid, actions: &mut Vec<Action>) {
    let selected = repo
        .list
        .selected_commit()
        .filter(|c| !repo.worktree_selected && c.id != id);
    let Some(selected) = selected else {
        return;
    };
    let hint = tr!(
        "compare-with-selected-hint",
        selected = selected.id.short(),
        id = id.short()
    );
    if ui
        .button(tr!("compare-with-selected"))
        .on_hover_text(hint)
        .clicked()
    {
        actions.push(Action::Compare(compare::Action::With(index)));
        ui.close();
    }
    ui.separator();
}

/// The search bar above the list: field, the toggles "Content", "Regex" and "Aa", number of
/// matches (or spinner and "Stop" while a content search runs), previous/next, close. Enter and
/// Shift+Enter in the field move between the matches (content: Enter searches), Esc closes the bar.
fn search_bar(ui: &mut Ui, search: &SearchBox, focus: bool, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.label(icons::text(icons::SEARCH, 14.0).weak());
        let mut text = search.text().to_owned();
        let options = search.options();
        // Room on the right for the toggles, the count and the three buttons.
        let width = (ui.available_width() - 420.0).max(120.0);
        let hint = if options.content {
            tr!("search-hint-content")
        } else {
            tr!("search-hint")
        };
        let edit = egui::TextEdit::singleline(&mut text)
            .id_salt("search-field")
            .hint_text(hint)
            .desired_width(width);
        let response = ui.add(edit);
        if focus {
            response.request_focus();
        }
        if response.changed() {
            actions.push(Action::Search(search::Action::Text(
                text,
                ui.input(|i| i.time),
            )));
        }
        let (escape, enter, shift) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Enter),
                i.modifiers.shift,
            )
        });
        if response.lost_focus() && escape {
            actions.push(Action::Search(search::Action::Close));
        } else if response.lost_focus() && enter {
            actions.push(if shift {
                Action::Search(search::Action::Previous)
            } else {
                Action::Search(search::Action::Next)
            });
            // The field keeps the keyboard: Enter again goes to the following match.
            response.request_focus();
        }
        search_options(ui, options, actions);
        if search.running() {
            ui.add(egui::Spinner::new().size(12.0));
            if ui
                .button(tr!("search-stop"))
                .on_hover_text(tr!("search-stop-tip"))
                .clicked()
            {
                actions.push(Action::Search(search::Action::Stop));
            }
        }
        let status = match search.state() {
            SearchState::Idle => RichText::new(""),
            SearchState::Searching => RichText::new(tr!("search-searching")).weak(),
            SearchState::Ready => RichText::new(tr!("search-press-enter")).weak(),
            SearchState::Stopped => RichText::new(tr!("search-stopped")).weak(),
            SearchState::NoMatches => {
                RichText::new(tr!("search-no-matches")).color(ui.visuals().warn_fg_color)
            }
            SearchState::Matches {
                current: Some(current),
                total,
            } => RichText::new(tr!("search-position", n = current + 1, total = total)),
            SearchState::Matches {
                current: None,
                total,
            } => RichText::new(tr!("search-count", total = total)),
            SearchState::Found {
                current: Some(current),
                count,
                more,
            } => RichText::new(if more {
                tr!("search-position-more", n = current + 1, total = count)
            } else {
                tr!("search-position", n = current + 1, total = count)
            }),
            SearchState::Found {
                current: None,
                count,
                more,
            } => RichText::new(if more {
                tr!("search-found-more", count = count)
            } else {
                tr!("search-found", count = count)
            }),
            SearchState::Failed { error, inline } => {
                RichText::new(if inline {
                    // The reason only, without "invalid parameters for `repo.search`: ".
                    error.split_once("`: ").map_or(error, |(_, r)| r).to_owned()
                } else {
                    tr!("search-failed")
                })
                .color(ui.visuals().error_fg_color)
            }
        };
        // Room for the three buttons: a long error is cut, its whole text on hover.
        let room = (ui.available_width() - 90.0).max(40.0);
        let label = ui
            .scope(|ui| {
                ui.set_max_width(room);
                ui.add(egui::Label::new(status).truncate())
            })
            .inner;
        if let SearchState::Failed { error, .. } = search.state() {
            label.on_hover_text(error);
        }
        let previous = ui
            .add_enabled(search.can_previous(), egui::Button::new("↑"))
            .on_hover_text(tr!("search-previous"));
        if previous.clicked() {
            actions.push(Action::Search(search::Action::Previous));
        }
        let next = ui
            .add_enabled(search.can_next(), egui::Button::new("↓"))
            .on_hover_text(tr!("search-next"));
        if next.clicked() {
            actions.push(Action::Search(search::Action::Next));
        }
        let close = ui
            .add(egui::Button::new(icons::text(icons::CLOSE, 12.0)).frame(false))
            .on_hover_text(tr!("search-close"));
        if close.clicked() {
            actions.push(Action::Search(search::Action::Close));
        }
    });
    ui.separator();
}

/// The toggles "Content", "Regex" and "Aa" (the last two only for content).
fn search_options(ui: &mut Ui, options: search::Options, actions: &mut Vec<Action>) {
    let mut next = options;
    ui.toggle_value(&mut next.content, tr!("search-content"))
        .on_hover_text(tr!("search-content-tip"));
    ui.add_enabled_ui(options.content, |ui| {
        ui.toggle_value(&mut next.regex, tr!("search-regex"))
            .on_hover_text(tr!("search-regex-tip"));
        ui.toggle_value(&mut next.ignore_case, "Aa")
            .on_hover_text(tr!("search-ignore-case-tip"));
    });
    if next != options {
        actions.push(Action::Search(search::Action::Options(
            next,
            ui.input(|i| i.time),
        )));
    }
}

/// The stroke from the changes ring down to HEAD's dot, if HEAD's column is
/// free in the rows in between (otherwise it would cover the edges of other commits).
pub(super) fn paint_changes_link(
    ui: &Ui,
    header: egui::Rect,
    anchor: Anchor,
    width: f32,
    step: f32,
    color: Color32,
) {
    if !anchor.free_above {
        return;
    }
    let x = graph::column_x(header.left(), anchor.column);
    let top = header.center().y + graph::NODE_RADIUS;
    let bottom = header.center().y + (anchor.index + 1) as f32 * step - graph::NODE_RADIUS;
    let column = egui::Rect::from_x_y_ranges(
        header.left()..=header.left() + width,
        ui.clip_rect().y_range(),
    );
    let painter = ui
        .painter()
        .with_clip_rect(column.intersect(ui.clip_rect()));
    painter.add(egui::Shape::dashed_line(
        &[egui::pos2(x, top), egui::pos2(x, bottom)],
        egui::Stroke::new(graph::EDGE_WIDTH, color),
        3.0,
        3.0,
    ));
}

/// The "Uncommitted changes" row: an empty ring in the graph (in HEAD's column `column`),
/// the title and the counts.
pub(super) fn changes_row(
    ui: &Ui,
    rect: egui::Rect,
    response: &egui::Response,
    repo: &RepoView,
    column: usize,
    graph_w: f32,
    style: &RowStyle,
) {
    if repo.worktree_selected {
        ui.painter()
            .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let center = egui::pos2(graph::column_x(rect.left(), column), rect.center().y);
    ui.painter().circle_stroke(
        center,
        graph::NODE_RADIUS,
        egui::Stroke::new(graph::EDGE_WIDTH, style.weak),
    );
    let painter = ui.painter_at(rect);
    let x = rect.left() + graph_w.max(graph::graph_width(1)) + 8.0;
    let title = painter.layout_no_wrap(tr!("commit-list-changes"), style.font.clone(), style.text);
    let width = title.size().x;
    painter.galley(
        egui::pos2(x, rect.center().y - title.size().y / 2.0),
        title,
        style.text,
    );
    let count = repo.status.as_ref().map_or(0, stage::change_count);
    let staged = repo.stage.staged.len();
    let detail = if staged > 0 {
        tr!("commit-list-changes-staged", n = count, staged = staged)
    } else {
        tr!("commit-list-changes-count", n = count)
    };
    painter.text(
        egui::pos2(x + width + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        detail,
        style.font.clone(),
        style.weak,
    );
    response
        .clone()
        .on_hover_text(tr!("commit-list-changes-hint"));
}

/// Edges and dots of the graph for rows `rows` (index and rectangle), in the column of width
/// `width` to the left of the rows. Each row draws the strokes towards the next one; the first
/// visible one also draws those coming to it from the row above, out of view.
pub(super) fn paint_graph(
    ui: &Ui,
    list: &CommitList,
    colors: &graph::LaneColors,
    rows: &[(usize, egui::Rect)],
    width: f32,
    step: f32,
) {
    let Some(&(first, first_rect)) = rows.first() else {
        return;
    };
    let layout = list.graph();
    let left = first_rect.left();
    let column = egui::Rect::from_x_y_ranges(left..=left + width, ui.clip_rect().y_range());
    let painter = ui
        .painter()
        .with_clip_rect(column.intersect(ui.clip_rect()));

    let previous = first
        .checked_sub(1)
        .map(|i| (i, first_rect.center().y - step));
    let current = rows.iter().map(|&(i, rect)| (i, rect.center().y));
    for (index, y) in previous.into_iter().chain(current) {
        let Some(row) = layout.rows.get(index) else {
            continue;
        };
        for (n, segment) in row.segments.iter().enumerate() {
            let stroke = egui::Stroke::new(graph::EDGE_WIDTH, colors.segment(index, n));
            match graph::segment_edge(segment, left, y, step) {
                graph::Edge::Line(points) => {
                    painter.line_segment(points, stroke);
                }
                graph::Edge::Curve(points) => {
                    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                        points,
                        false,
                        Color32::TRANSPARENT,
                        stroke,
                    ));
                }
            }
        }
    }

    // The dots above the edges. A merge is a ring: you can see right away where branches join.
    for &(index, rect) in rows {
        let (Some(row), Some(commit)) = (layout.rows.get(index), list.commits().get(index)) else {
            continue;
        };
        let center = egui::pos2(graph::column_x(left, row.column), rect.center().y);
        let color = colors.commit(index);
        if commit.is_merge() {
            painter.circle(
                center,
                graph::NODE_RADIUS,
                ui.visuals().panel_fill,
                egui::Stroke::new(graph::EDGE_WIDTH, color),
            );
        } else {
            painter.circle_filled(center, graph::NODE_RADIUS, color);
        }
    }
}

/// The colors of the graph's lanes ([`graph::lane_colors`]), computed again only when the graph,
/// the refs or the remotes change.
fn lane_colors(ui: &Ui, repo: &RepoView) -> std::sync::Arc<graph::LaneColors> {
    type Key = (u64, u64, Vec<String>);
    let remotes = repo.remotes.names();
    let key: Key = (repo.list.graph_version(), repo.refs_version, remotes);
    let id = ui.make_persistent_id(("lane-colors", &repo.path));
    if let Some((cached, colors)) =
        ui.data(|d| d.get_temp::<(Key, std::sync::Arc<graph::LaneColors>)>(id))
        && cached == key
    {
        return colors;
    }
    let ids: Vec<_> = repo.list.commits().iter().map(|c| c.id).collect();
    let colors = std::sync::Arc::new(graph::lane_colors(
        repo.list.graph(),
        &ids,
        &repo.labels,
        &key.2,
    ));
    ui.data_mut(|d| d.insert_temp(id, (key, colors.clone())));
    colors
}

/// The row (of the view) to bring into view, if it is not already fully visible.
pub(super) fn scroll_target(target: Option<usize>, viewport: &ListViewport) -> Option<usize> {
    let target = target?;
    let fully_visible = viewport.rows.start < target && target + 1 < viewport.rows.end;
    (!fully_visible).then_some(target)
}

/// Font and colors of the list rows, read once per frame.
#[derive(Clone)]
pub(super) struct RowStyle {
    font: FontId,
    mono: FontId,
    label: FontId,
    text: Color32,
    weak: Color32,
    tag_fill: Color32,
    head_fill: Color32,
    head_text: Color32,
    /// Dates relative to this moment (seconds since the epoch); `None`: date and time.
    now: Option<i64>,
}

/// Seconds since the epoch now (0 if the clock is before it).
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// Width of the id column.
const ID_W: f32 = 70.0;
/// Width of the date column.
const DATE_W: f32 = 120.0;
/// The subject keeps at least this much: in a narrow list the author goes first, then the date.
const SUMMARY_MIN: f32 = 220.0;

/// Widths of the author and date columns in a row `width` wide (0: hidden).
pub(super) fn side_columns(width: f32) -> (f32, f32) {
    let author = (width * 0.2).clamp(80.0, 180.0);
    let room = width - ID_W - SUMMARY_MIN;
    if room >= author + DATE_W {
        (author, DATE_W)
    } else if room >= DATE_W {
        (0.0, DATE_W)
    } else {
        (0.0, 0.0)
    }
}

/// The date of the list: relative to `now` if given, otherwise date and time.
fn commit_date(sig: &Signature, now: Option<i64>) -> String {
    match now {
        Some(now) => format::relative(sig, now),
        None => date_time(sig),
    }
}

/// Columns: id | ref labels and subject (elastic) | author | date. Returns where the `+N` of the
/// labels that did not fit is, if any.
pub(super) fn commit_row(
    ui: &Ui,
    rect: egui::Rect,
    commit: &Commit,
    labels: &[RefLabel],
    lane: Color32,
    style: &RowStyle,
) -> Option<egui::Rect> {
    let (author_w, date_w) = side_columns(rect.width());
    let summary_w = (rect.width() - ID_W - author_w - date_w).max(40.0);
    let columns = [
        (ID_W, commit.id.short(), &style.mono, style.weak),
        (summary_w, commit.summary.clone(), &style.font, style.text),
        (
            author_w,
            commit.author.name.clone(),
            &style.font,
            style.weak,
        ),
        (
            date_w,
            commit_date(&commit.author, style.now),
            &style.font,
            style.weak,
        ),
    ];
    let mut x = rect.left() + 8.0;
    let mut more = None;
    for (i, (width, value, font, color)) in columns.into_iter().enumerate() {
        if width <= 0.0 {
            continue;
        }
        let cell =
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(width - 8.0, rect.height()));
        let painter = ui.painter_at(cell);
        let mut start = cell.left_top();
        if i == 1 {
            // The subject keeps its first characters; the labels take the rest.
            let summary = painter.layout_no_wrap(value.clone(), font.clone(), color);
            let prefix: String = value.chars().take(SUMMARY_CHARS).chain(['…']).collect();
            let prefix = painter.layout_no_wrap(prefix, font.clone(), color);
            let room = cell.width() - summary.size().x.min(prefix.size().x) - LABEL_GAP;
            let (used, hidden) = ref_labels(&painter, cell, labels, lane, style, room);
            start.x += used;
            more = hidden;
        }
        let galley = elided(&painter, value, font.clone(), color, cell.right() - start.x);
        let top = cell.center().y - galley.size().y / 2.0;
        painter.galley(egui::pos2(start.x, top), galley, color);
        x += width;
    }
    more
}

/// Characters of the subject that stay before the ref labels collapse into "+N".
const SUMMARY_CHARS: usize = 8;

/// How many ref labels (`widths`, gap included) a row shows in `room`: all if they fit,
/// otherwise as many as fit next to the `+N` (`more` wide), and at least the first one.
pub(super) fn shown_labels(widths: &[f32], room: f32, more: f32) -> usize {
    graph::labels_that_fit(widths, room, more).max(widths.len().min(1))
}

/// `text` on one line, cut with "…" if wider than `width`.
pub(super) fn elided(
    painter: &egui::Painter,
    text: String,
    font: FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    painter.layout_job(job)
}

/// The tooltip of the `+N` of a row: every label, one per line.
pub(super) fn labels_tooltip(labels: &[RefLabel]) -> String {
    labels
        .iter()
        .map(|label| label.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Space between one label and the next, and horizontal inner margin.
pub(super) const LABEL_GAP: f32 = 4.0;

pub(super) const LABEL_PAD: f32 = 4.0;

/// Draws the ref labels at the start of `cell`, in the color of the commit's lane, and
/// returns the width used and where the `+N` is. They take at most `room`; those that do not
/// fit become `+N`, but the first one is always drawn.
pub(super) fn ref_labels(
    painter: &egui::Painter,
    cell: egui::Rect,
    labels: &[RefLabel],
    lane: Color32,
    style: &RowStyle,
    room: f32,
) -> (f32, Option<egui::Rect>) {
    if labels.is_empty() {
        return (0.0, None);
    }
    let look = |kind: LabelKind| -> (Color32, egui::Stroke, Color32) {
        let on_badge = theme::ON_BADGE;
        // Outlines and text in the lane's color must read on the panel too.
        let outline = theme::as_text(lane, painter.ctx().global_style().visuals.dark_mode);
        match kind {
            LabelKind::Head => (style.head_fill, egui::Stroke::NONE, style.head_text),
            LabelKind::CurrentBranch => (lane, egui::Stroke::new(1.5, style.text), on_badge),
            LabelKind::Branch => (lane, egui::Stroke::NONE, on_badge),
            LabelKind::Remote => (
                Color32::TRANSPARENT,
                egui::Stroke::new(1.0, outline),
                outline,
            ),
            LabelKind::Tag => (
                style.tag_fill,
                egui::Stroke::new(1.0, style.weak),
                style.text,
            ),
            // Stashes: only the border, gray, in the dimmed text: they are not branches.
            LabelKind::Stash => (
                Color32::TRANSPARENT,
                egui::Stroke::new(1.0, style.weak),
                style.weak,
            ),
        }
    };
    let galleys: Vec<_> = labels
        .iter()
        .map(|label| {
            let (_, _, color) = look(label.kind);
            painter.layout_no_wrap(label.text.clone(), style.label.clone(), color)
        })
        .collect();
    let widths: Vec<f32> = galleys
        .iter()
        .map(|g| g.size().x + 2.0 * LABEL_PAD + LABEL_GAP)
        .collect();
    let hidden_text = |n: usize| format!("+{n}");
    let more = painter.layout_no_wrap(hidden_text(labels.len()), style.label.clone(), style.weak);
    let shown = shown_labels(&widths, room, more.size().x + LABEL_GAP);

    let height = style.label.size + 4.0;
    let mut x = cell.left();
    for ((label, galley), width) in labels.iter().zip(galleys).zip(&widths).take(shown) {
        let (fill, stroke, color) = look(label.kind);
        let pill = egui::Rect::from_min_size(
            egui::pos2(x, cell.center().y - height / 2.0),
            vec2(width - LABEL_GAP, height),
        );
        painter.rect(pill, 3.0, fill, stroke, egui::StrokeKind::Inside);
        let top = pill.center().y - galley.size().y / 2.0;
        painter.galley(egui::pos2(pill.left() + LABEL_PAD, top), galley, color);
        x += width;
    }
    let mut more = None;
    if shown < labels.len() {
        let rest = painter.layout_no_wrap(
            hidden_text(labels.len() - shown),
            style.label.clone(),
            style.weak,
        );
        let top = cell.center().y - rest.size().y / 2.0;
        let width = rest.size().x + LABEL_GAP;
        more = Some(egui::Rect::from_min_size(egui::pos2(x, top), rest.size()));
        painter.galley(egui::pos2(x, top), rest, style.weak);
        x += width;
    }
    (x - cell.left(), more)
}

/// "Cherry-pick onto «branch»" and "Revert" of the commit menu: off like merge (detached
/// HEAD, operation in progress, write or open dialog). For a merge commit the parent's
/// dialog is opened first. `cherry_pick`: false for a commit already in HEAD, which only
/// gets "Revert".
fn pick_items(
    ui: &mut Ui,
    commit: &Commit,
    what: &str,
    gate: &merge::Gate,
    cherry_pick: bool,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    ui.separator();
    let active = tr!("commit-current-branch");
    let (into, blocked) = match gate {
        merge::Gate::Into(branch) => (branch.as_str(), None),
        merge::Gate::Blocked(reason) => (active.as_str(), Some(reason.as_str())),
    };
    let items = if commit.parents.len() > 1 {
        [
            (
                merge::Pick::CherryPick,
                tr!("commit-cherry-pick-merge-item", into = into),
                tr!("commit-cherry-pick-merge-hint", into = into, what = what),
            ),
            (
                merge::Pick::Revert,
                tr!("commit-revert-merge-item"),
                tr!("commit-revert-merge-hint", into = into, what = what),
            ),
        ]
    } else {
        [
            (
                merge::Pick::CherryPick,
                tr!("commit-cherry-pick-item", into = into),
                tr!("commit-cherry-pick-hint", into = into, what = what),
            ),
            (
                merge::Pick::Revert,
                tr!("commit-revert-item"),
                tr!("commit-revert-hint", into = into, what = what),
            ),
        ]
    };
    for (pick, label, hint) in items {
        if pick == merge::Pick::CherryPick && !cherry_pick {
            continue;
        }
        let button = ui
            .add_enabled(can_write && blocked.is_none(), egui::Button::new(label))
            .on_hover_text(hint)
            .on_disabled_hover_text(blocked.map_or_else(wait, str::to_owned));

        if button.clicked() {
            actions.push(Action::Merge(merge::Action::Pick {
                pick,
                rev: commit.id,
                what: what.to_owned(),
                parents: commit.parents.clone(),
            }));
            ui.close();
        }
    }
}

/// "Interactive rebase from here…": base = the parent of `commit`, which must be on the first-parent
/// history of HEAD among the loaded commits (the engine checks again: merges in the range, ...).
fn rebase_item(
    ui: &mut Ui,
    repo: &RepoView,
    commit: &Commit,
    what: &str,
    gate: &merge::Gate,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    ui.separator();
    let blocked = match gate {
        merge::Gate::Blocked(reason) => Some(reason.clone()),
        merge::Gate::Into(_) => match commit.parents.first() {
            None => Some(tr!("rebase-menu-root", id = commit.id.short())),
            Some(_) if !on_first_parent_history(repo, commit.id) => {
                Some(tr!("rebase-menu-not-on-branch"))
            }
            Some(_) => None,
        },
    };
    let branch = match gate {
        merge::Gate::Into(branch) => branch.as_str(),
        merge::Gate::Blocked(_) => "",
    };
    let button = ui
        .add_enabled(
            can_write && blocked.is_none(),
            egui::Button::new(tr!("rebase-menu-item")),
        )
        .on_hover_text(tr!(
            "rebase-menu-hint",
            id = commit.id.short(),
            branch = branch
        ))
        .on_disabled_hover_text(blocked.unwrap_or_else(wait));
    if button.clicked()
        && let Some(&base) = commit.parents.first()
    {
        actions.push(Action::Rebase(rebase::Action::Open {
            base,
            what: what.to_owned(),
        }));
        ui.close();
    }
}

/// True if `id` is HEAD or one of its ancestors among the loaded commits (all parents). Only
/// computed while the commit menu is open, on the list already loaded: no git call.
fn in_head_history(repo: &RepoView, id: Oid) -> bool {
    let Some(head) = repo.head.target() else {
        return false;
    };
    let by_id: std::collections::HashMap<Oid, &Commit> =
        repo.list.commits().iter().map(|c| (c.id, c)).collect();
    let mut seen = std::collections::HashSet::new();
    let mut next = vec![head];
    while let Some(current) = next.pop() {
        if current == id {
            return true;
        }
        if seen.insert(current)
            && let Some(commit) = by_id.get(&current)
        {
            next.extend(commit.parents.iter().copied());
        }
    }
    false
}

/// True if `id` is reached from HEAD following first parents among the loaded commits.
fn on_first_parent_history(repo: &RepoView, id: Oid) -> bool {
    let commits = repo.list.commits();
    let by_id: std::collections::HashMap<Oid, &Commit> =
        commits.iter().map(|c| (c.id, c)).collect();
    let mut next = repo.head.target();
    while let Some(current) = next {
        if current == id {
            return true;
        }
        next = by_id.get(&current).and_then(|c| c.parents.first().copied());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_labels_collapse_to_the_first_one_and_the_count() {
        assert_eq!(shown_labels(&[40.0, 40.0], 100.0, 20.0), 2);
        assert_eq!(shown_labels(&[40.0, 40.0, 40.0], 70.0, 20.0), 1);
        // Too narrow even for one: the first stays, the others go into "+N".
        assert_eq!(shown_labels(&[80.0, 40.0], 30.0, 20.0), 1);
        assert_eq!(shown_labels(&[], 30.0, 20.0), 0);
    }

    #[test]
    fn a_narrow_list_hides_the_author_then_the_date() {
        assert_eq!(side_columns(900.0), (180.0, DATE_W));
        assert_eq!(side_columns(600.0), (120.0, DATE_W));
        assert_eq!(side_columns(500.0), (0.0, DATE_W));
        assert_eq!(side_columns(420.0), (0.0, DATE_W));
        assert_eq!(side_columns(300.0), (0.0, 0.0));
    }
}
