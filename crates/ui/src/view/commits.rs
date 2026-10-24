use super::*;

/// Height of a row of the commit list.
pub(super) const ROW_HEIGHT: f32 = 22.0;

/// `writing`: a write is in progress, the menu entries that write wait.
pub(crate) fn commit_list(
    ui: &mut Ui,
    repo: &RepoView,
    viewport: &mut ListViewport,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    let can_write = !writing && !repo.modal_open();
    let merge = merge::Gate::new(&repo.head, repo.status.as_ref());
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
    let style = RowStyle {
        font: FontId::proportional(13.5),
        mono: FontId::monospace(12.5),
        label: FontId::proportional(11.5),
        text: ui.visuals().text_color(),
        weak: ui.visuals().weak_text_color(),
        tag_fill: ui.visuals().widgets.inactive.bg_fill,
        head_fill: ui.visuals().strong_text_color(),
        head_text: ui.visuals().panel_fill,
    };
    // One more row at the bottom while there are more commits to load.
    let rows = offset + list.commits().len() + usize::from(list.has_more());
    let step = ROW_HEIGHT + ui.spacing().item_spacing.y;
    // The graph takes at most two fifths of the list: beyond that it is cut on the right.
    let graph_w = graph::graph_width(list.graph().width).min(ui.available_width() * 0.4);

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
                changes_row(ui, rect, &response, repo, column, graph_w, &style);
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
            if list.selected() == Some(index) && !repo.worktree_selected {
                ui.painter()
                    .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            if response.clicked() {
                actions.push(Action::Select(index));
            }
            let id = commit.id;
            response.context_menu(|ui| {
                let button = ui
                    .add_enabled(can_write, egui::Button::new(tr!("commit-new-branch")))
                    .on_hover_text(tr!("commit-new-branch-hint", id = id.short()))
                    .on_disabled_hover_text(wait());
                if button.clicked() {
                    actions.push(Action::NewBranch(Start::from_commit(id), String::new()));
                    ui.close();
                }
                let what = merge::commit_name(id, &repo.refs);
                merge_items(ui, &id.to_string(), &what, &merge, can_write, actions);
                pick_items(ui, commit, &what, &merge, can_write, actions);
                rebase_item(ui, repo, commit, &what, &merge, can_write, actions);
            });
            let lane = list.graph().rows.get(index).map_or(0, |row| row.column);
            let labels = repo.labels.get(&commit.id).map_or(&[][..], Vec::as_slice);
            let text_rect = rect.with_min_x(rect.left() + graph_w);
            commit_row(
                ui,
                text_rect,
                commit,
                labels,
                graph::lane_color(lane),
                &style,
            );
            drawn.push((index, rect));
        }
        if let (Some(rect), Some(anchor)) = (header_rect, anchor) {
            paint_changes_link(ui, rect, anchor, graph_w, step, style.weak);
        }
        paint_graph(ui, list, &drawn, graph_w, step);
        range
    });
    viewport.rows = output.inner;
    viewport.height = output.inner_rect.height();
    actions.push(Action::Visible(viewport.rows.end.saturating_sub(offset)));
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
        for segment in &row.segments {
            let stroke = egui::Stroke::new(graph::EDGE_WIDTH, graph::lane_color(segment.lane));
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
        let color = graph::lane_color(row.column);
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

/// The row (of the view) to bring into view, if it is not already fully visible.
pub(super) fn scroll_target(target: Option<usize>, viewport: &ListViewport) -> Option<usize> {
    let target = target?;
    let fully_visible = viewport.rows.start < target && target + 1 < viewport.rows.end;
    (!fully_visible).then_some(target)
}

/// Font and colors of the list rows, read once per frame.
pub(super) struct RowStyle {
    font: FontId,
    mono: FontId,
    label: FontId,
    text: Color32,
    weak: Color32,
    tag_fill: Color32,
    head_fill: Color32,
    head_text: Color32,
}

/// Columns: id | ref labels and subject (elastic) | author | date.
pub(super) fn commit_row(
    ui: &Ui,
    rect: egui::Rect,
    commit: &Commit,
    labels: &[RefLabel],
    lane: Color32,
    style: &RowStyle,
) {
    let date_w = 120.0;
    let author_w = (rect.width() * 0.2).clamp(80.0, 180.0);
    let id_w = 70.0;
    let summary_w = (rect.width() - id_w - author_w - date_w).max(40.0);
    let columns = [
        (id_w, commit.id.short(), &style.mono, style.weak),
        (summary_w, commit.summary.clone(), &style.font, style.text),
        (
            author_w,
            commit.author.name.clone(),
            &style.font,
            style.weak,
        ),
        (date_w, date_time(&commit.author), &style.font, style.weak),
    ];
    let mut x = rect.left() + 8.0;
    for (i, (width, value, font, color)) in columns.into_iter().enumerate() {
        let cell =
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(width - 8.0, rect.height()));
        let painter = ui.painter_at(cell);
        let mut start = cell.left_center();
        if i == 1 {
            start.x += ref_labels(&painter, cell, labels, lane, style);
        }
        painter.text(start, Align2::LEFT_CENTER, value, font.clone(), color);
        x += width;
    }
}

/// Space between one label and the next, and horizontal inner margin.
pub(super) const LABEL_GAP: f32 = 4.0;

pub(super) const LABEL_PAD: f32 = 4.0;

/// Draws the ref labels at the start of `cell`, in the color of the commit's lane, and
/// returns the width used. They take at most half the cell; those that do not fit
/// become `+N`.
pub(super) fn ref_labels(
    painter: &egui::Painter,
    cell: egui::Rect,
    labels: &[RefLabel],
    lane: Color32,
    style: &RowStyle,
) -> f32 {
    if labels.is_empty() {
        return 0.0;
    }
    let look = |kind: LabelKind| -> (Color32, egui::Stroke, Color32) {
        let dark = Color32::from_gray(20);
        match kind {
            LabelKind::Head => (style.head_fill, egui::Stroke::NONE, style.head_text),
            LabelKind::CurrentBranch => (lane, egui::Stroke::new(1.5, style.text), dark),
            LabelKind::Branch => (lane, egui::Stroke::NONE, dark),
            LabelKind::Remote => (Color32::TRANSPARENT, egui::Stroke::new(1.0, lane), lane),
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
    let shown = graph::labels_that_fit(&widths, cell.width() / 2.0, more.size().x + LABEL_GAP);

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
    if shown < labels.len() {
        let rest = painter.layout_no_wrap(
            hidden_text(labels.len() - shown),
            style.label.clone(),
            style.weak,
        );
        let top = cell.center().y - rest.size().y / 2.0;
        let width = rest.size().x + LABEL_GAP;
        painter.galley(egui::pos2(x, top), rest, style.weak);
        x += width;
    }
    x - cell.left()
}

/// "Cherry-pick onto «branch»" and "Revert" of the commit menu: off like merge (detached
/// HEAD, operation in progress, write or open dialog). For a merge commit the parent's
/// dialog is opened first.
fn pick_items(
    ui: &mut Ui,
    commit: &Commit,
    what: &str,
    gate: &merge::Gate,
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
        let button = ui
            .add_enabled(can_write && blocked.is_none(), egui::Button::new(label))
            .on_hover_text(hint)
            .on_disabled_hover_text(blocked.map_or_else(wait, str::to_owned));

        if button.clicked() {
            actions.push(Action::Pick {
                pick,
                rev: commit.id,
                what: what.to_owned(),
                parents: commit.parents.clone(),
            });
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
        actions.push(Action::OpenRebase {
            base,
            what: what.to_owned(),
        });
        ui.close();
    }
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
