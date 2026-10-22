//! Disegno dei pannelli. Le funzioni leggono la vista e producono [`Action`]; non cambiano
//! niente direttamente.

use crate::format::{date_time, date_time_zone};
use crate::graph::{self, LabelKind, RefLabel};
use crate::list::CommitList;
use crate::{Action, Job, ListViewport, RepoView};
use egui::{Align2, Color32, FontId, RichText, Sense, Ui, vec2};
use project_domain::{Commit, Head, RefInfo, RefKind, Signature, Upstream};

/// Ultimo componente del percorso, per il titolo.
fn repo_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

fn head_text(head: &Head) -> String {
    match head {
        Head::Branch { name, target: None } => format!("{name} (senza commit)"),
        Head::Branch { name, .. } => name.clone(),
        Head::Detached(id) => format!("HEAD staccata su {}", id.short()),
    }
}

/// `+2 -1` (commit avanti e indietro), vuoto se in pari; `upstream sparito` se il ref remoto
/// non c'è più. Solo caratteri ASCII: i font di default non hanno le frecce su tutte le
/// piattaforme.
fn ahead_behind(upstream: &Upstream) -> String {
    match (upstream.ahead, upstream.behind) {
        (Some(a), Some(b)) => [(a, '+'), (b, '-')]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, sign)| format!("{sign}{n}"))
            .collect::<Vec<_>>()
            .join(" "),
        _ => "upstream sparito".to_owned(),
    }
}

pub(crate) fn top_bar(
    ui: &mut Ui,
    repo: Option<&RepoView>,
    can_pick: bool,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        if can_pick && ui.button("Apri…").clicked() {
            actions.push(Action::PickFolder);
        }
        let Some(repo) = repo else {
            return;
        };
        if ui.button("Aggiorna").clicked() {
            actions.push(Action::Refresh);
        }
        ui.separator();
        ui.strong(repo_name(&repo.path)).on_hover_text(&repo.path);
        ui.separator();
        ui.label(head_text(&repo.head));
        if let Some(status) = &repo.status {
            if let Some(upstream) = &status.upstream {
                let text = ahead_behind(upstream);
                if !text.is_empty() {
                    ui.weak(text).on_hover_text(format!(
                        "commit avanti (+) e indietro (-) rispetto a {}",
                        upstream.name
                    ));
                }
            }
            let changes = status.entries.len();
            if changes > 0 {
                ui.separator();
                ui.label(format!("{changes} file modificati"));
            }
        }
    });
}

pub(crate) fn status_bar(ui: &mut Ui, running: &[Job], error: &str) {
    ui.horizontal(|ui| {
        if let Some(job) = running.first() {
            ui.spinner();
            let text = match running.len() - 1 {
                0 => format!("{} in corso…", job.command),
                n => format!("{} in corso… (+{n} in coda)", job.command),
            };
            ui.label(text);
        } else if !error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, error);
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
            ui.label("Apertura del repository…");
        } else if can_pick {
            ui.heading("Nessun repository aperto");
            ui.add_space(8.0);
            if ui.button("Apri un repository…").clicked() {
                actions.push(Action::PickFolder);
            }
        } else {
            ui.heading("Nessun repository aperto");
        }
    });
}

pub(crate) fn refs_panel(ui: &mut Ui, repo: &RepoView, actions: &mut Vec<Action>) {
    let current = match &repo.head {
        Head::Branch { name, .. } => Some(format!("refs/heads/{name}")),
        Head::Detached(_) => None,
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        let sections = [
            (RefKind::LocalBranch, "Branch locali"),
            (RefKind::RemoteBranch, "Branch remoti"),
            (RefKind::Tag, "Tag"),
            (RefKind::Stash, "Stash"),
        ];
        for (kind, title) in sections {
            let refs: Vec<&RefInfo> = repo.refs.iter().filter(|r| r.kind == kind).collect();
            if refs.is_empty() {
                continue;
            }
            egui::CollapsingHeader::new(format!("{title} ({})", refs.len()))
                .default_open(kind != RefKind::Tag)
                .show(ui, |ui| {
                    for info in refs {
                        ref_row(ui, info, current.as_deref() == Some(&info.name), actions);
                    }
                });
        }
    });
}

fn ref_row(ui: &mut Ui, info: &RefInfo, is_head: bool, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        // Il branch corrente è evidenziato e in grassetto.
        let name = RichText::new(info.short_name());
        let name = if is_head { name.strong() } else { name };
        let response = ui.selectable_label(is_head, name);
        if response.clicked() {
            actions.push(Action::SelectCommit(info.target));
        }
        if let Some(upstream) = &info.upstream {
            let text = ahead_behind(upstream);
            if !text.is_empty() {
                ui.weak(text);
            }
        }
    })
    .response
    .on_hover_text(format!("{}\n{}", info.name, info.target));
}

/// Altezza di una riga della lista dei commit.
const ROW_HEIGHT: f32 = 22.0;

pub(crate) fn commit_list(
    ui: &mut Ui,
    repo: &RepoView,
    viewport: &mut ListViewport,
    actions: &mut Vec<Action>,
) {
    let list = &repo.list;
    if list.commits().is_empty() {
        let text = if list.is_loading() {
            "Caricamento dei commit…"
        } else {
            "Nessun commit."
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
    // Una riga in più in fondo mentre ci sono altri commit da caricare.
    let rows = list.commits().len() + usize::from(list.has_more());
    let step = ROW_HEIGHT + ui.spacing().item_spacing.y;
    // Il grafo prende al massimo due quinti della lista: oltre si taglia a destra.
    let graph_w = graph::graph_width(list.graph().width).min(ui.available_width() * 0.4);

    let mut area = egui::ScrollArea::vertical().auto_shrink(false);
    if let Some(target) = scroll_target(list, viewport) {
        let top = target as f32 * step;
        // Verso l'alto la riga va in cima, verso il basso in fondo alla vista.
        let offset = if target <= viewport.rows.start {
            top
        } else {
            top + step - viewport.height
        };
        area = area.vertical_scroll_offset(offset.max(0.0));
    }

    let output = area.show_rows(ui, ROW_HEIGHT, rows, |ui, range| {
        // Righe di commit disegnate: il grafo va sopra gli sfondi di tutte, quindi dopo.
        let mut drawn = Vec::with_capacity(range.len());
        for index in range.clone() {
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
            let Some(commit) = list.commits().get(index) else {
                ui.painter().text(
                    rect.left_center() + vec2(graph_w + 8.0, 0.0),
                    Align2::LEFT_CENTER,
                    "Caricamento di altri commit…",
                    style.font.clone(),
                    style.weak,
                );
                continue;
            };
            if list.selected() == Some(index) {
                ui.painter()
                    .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            if response.clicked() {
                actions.push(Action::Select(index));
            }
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
        paint_graph(ui, list, &drawn, graph_w, step);
        range
    });
    viewport.rows = output.inner;
    viewport.height = output.inner_rect.height();
    actions.push(Action::Visible(viewport.rows.end));
}

/// Archi e pallini del grafo per le righe `rows` (indice e rettangolo), nella colonna larga
/// `width` a sinistra delle righe. Ogni riga disegna i tratti verso la successiva; la prima
/// visibile disegna anche quelli che le arrivano dalla riga sopra, fuori vista.
fn paint_graph(ui: &Ui, list: &CommitList, rows: &[(usize, egui::Rect)], width: f32, step: f32) {
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

    // I pallini sopra gli archi. Un merge è un anello: si vede subito dove si uniscono i rami.
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

/// La riga da portare in vista, se non è già tutta visibile.
fn scroll_target(list: &CommitList, viewport: &ListViewport) -> Option<usize> {
    let target = list.peek_scroll_to()?;
    let fully_visible = viewport.rows.start < target && target + 1 < viewport.rows.end;
    (!fully_visible).then_some(target)
}

/// Font e colori delle righe della lista, letti una volta per frame.
struct RowStyle {
    font: FontId,
    mono: FontId,
    label: FontId,
    text: Color32,
    weak: Color32,
    tag_fill: Color32,
    head_fill: Color32,
    head_text: Color32,
}

/// Colonne: id | etichette dei ref e oggetto (elastico) | autore | data.
fn commit_row(
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

/// Spazio tra un'etichetta e l'altra, e margine interno orizzontale.
const LABEL_GAP: f32 = 4.0;
const LABEL_PAD: f32 = 4.0;

/// Disegna le etichette dei ref all'inizio di `cell`, nel colore della corsia del commit, e
/// restituisce la larghezza occupata. Prendono al massimo metà della cella; quelle che non ci
/// stanno diventano `+N`.
fn ref_labels(
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

fn signer(sig: &Signature) -> String {
    format!("{} <{}>", sig.name, sig.email)
}

pub(crate) fn details_panel(ui: &mut Ui, commit: Option<&Commit>) {
    let Some(commit) = commit else {
        ui.centered_and_justified(|ui| ui.weak("Nessun commit selezionato."));
        return;
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(&commit.summary).heading()).wrap());
        if !commit.body.is_empty() {
            ui.add_space(4.0);
            ui.add(egui::Label::new(&commit.body).wrap());
        }
        ui.add_space(8.0);
        ui.separator();
        egui::Grid::new("dettagli-commit")
            .num_columns(2)
            .spacing(vec2(12.0, 4.0))
            .show(ui, |ui| {
                ui.weak("Commit");
                ui.add(egui::Label::new(RichText::new(commit.id.to_string()).monospace()).wrap());
                ui.end_row();
                let parents = match commit.parents.len() {
                    0 => "Radice",
                    1 => "Genitore",
                    _ => "Genitori",
                };
                ui.weak(parents);
                ui.vertical(|ui| {
                    for parent in &commit.parents {
                        ui.monospace(parent.short());
                    }
                });
                ui.end_row();
                ui.weak("Autore");
                ui.add(egui::Label::new(signer(&commit.author)).wrap());
                ui.end_row();
                ui.weak("");
                ui.weak(date_time_zone(&commit.author));
                ui.end_row();
                if commit.committer != commit.author {
                    ui.weak("Committer");
                    ui.add(egui::Label::new(signer(&commit.committer)).wrap());
                    ui.end_row();
                    ui.weak("");
                    ui.weak(date_time_zone(&commit.committer));
                    ui.end_row();
                }
            });
        if commit.is_merge() {
            ui.add_space(8.0);
            ui.colored_label(Color32::from_rgb(0x9b, 0x6b, 0xdf), "Commit di merge");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_name_is_the_last_component() {
        assert_eq!(repo_name("/home/ada/progetto"), "progetto");
        assert_eq!(repo_name("/home/ada/progetto/"), "progetto");
        assert_eq!(repo_name(r"C:\repo\app"), "app");
        assert_eq!(repo_name("esempio"), "esempio");
        assert_eq!(repo_name("/"), "/");
    }

    #[test]
    fn ahead_behind_text() {
        let up = |ahead, behind| Upstream {
            name: "origin/main".to_owned(),
            ahead,
            behind,
        };
        assert_eq!(ahead_behind(&up(Some(0), Some(0))), "");
        assert_eq!(ahead_behind(&up(Some(2), Some(0))), "+2");
        assert_eq!(ahead_behind(&up(Some(2), Some(1))), "+2 -1");
        assert_eq!(ahead_behind(&up(None, None)), "upstream sparito");
    }
}
