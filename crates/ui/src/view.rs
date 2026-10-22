//! Disegno dei pannelli. Le funzioni leggono la vista e producono [`Action`]; non cambiano
//! niente direttamente.

use crate::details::Shown;
use crate::diff::{DiffSource, DiffTarget};
use crate::format::{self, change_letter, date_time, date_time_zone, file_path, file_score};
use crate::graph::{self, LabelKind, RefLabel};
use crate::list::CommitList;
use crate::stage::{self, PendingWrite, Side, StageItem, StageOp};
use crate::{Action, Job, ListViewport, RepoView};
use egui::{Align2, Color32, FontId, RichText, Sense, Ui, vec2};
use project_domain::{
    Change, ChangedFile, Commit, Conflict, Head, Oid, RefInfo, RefKind, Signature, Upstream,
};

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

/// `write` è la scrittura in corso, descritta: ha la precedenza sugli altri comandi.
pub(crate) fn status_bar(ui: &mut Ui, running: &[Job], write: Option<&str>, error: &str) {
    ui.horizontal(|ui| {
        if let Some(write) = write {
            ui.spinner();
            ui.label(format!("{write} in corso…"));
        } else if let Some(job) = running.first() {
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
    // La riga "Modifiche non committate" è la riga 0: i commit partono da `offset`.
    let header = repo.has_changes();
    let offset = usize::from(header);
    if list.commits().is_empty() && !header {
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
    let rows = offset + list.commits().len() + usize::from(list.has_more());
    let step = ROW_HEIGHT + ui.spacing().item_spacing.y;
    // Il grafo prende al massimo due quinti della lista: oltre si taglia a destra.
    let graph_w = graph::graph_width(list.graph().width).min(ui.available_width() * 0.4);

    let mut area = egui::ScrollArea::vertical().auto_shrink(false);
    let wanted = if repo.scroll_header {
        Some(0)
    } else {
        list.peek_scroll_to().map(|i| i + offset)
    };
    if let Some(target) = scroll_target(wanted, viewport) {
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
        for row in range.clone() {
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
            if header && row == 0 {
                changes_row(ui, rect, &response, repo, graph_w, &style);
                if response.clicked() {
                    actions.push(Action::SelectWorktree);
                }
                continue;
            }
            let index = row - offset;
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
    actions.push(Action::Visible(viewport.rows.end.saturating_sub(offset)));
}

/// La riga "Modifiche non committate": un anello vuoto nel grafo, il titolo e i conteggi.
fn changes_row(
    ui: &Ui,
    rect: egui::Rect,
    response: &egui::Response,
    repo: &RepoView,
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
    let center = egui::pos2(graph::column_x(rect.left(), 0), rect.center().y);
    ui.painter().circle_stroke(
        center,
        graph::NODE_RADIUS,
        egui::Stroke::new(graph::EDGE_WIDTH, style.weak),
    );
    let painter = ui.painter_at(rect);
    let x = rect.left() + graph_w.max(graph::graph_width(1)) + 8.0;
    let title = painter.layout_no_wrap(
        "Modifiche non committate".to_owned(),
        style.font.clone(),
        style.text,
    );
    let width = title.size().x;
    painter.galley(
        egui::pos2(x, rect.center().y - title.size().y / 2.0),
        title,
        style.text,
    );
    let count = repo.status.as_ref().map_or(0, stage::change_count);
    let staged = repo.stage.staged.len();
    let mut detail = match count {
        1 => "1 file".to_owned(),
        n => format!("{n} file"),
    };
    if staged > 0 {
        detail.push_str(&format!(", {staged} in stage"));
    }
    painter.text(
        egui::pos2(x + width + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        detail,
        style.font.clone(),
        style.weak,
    );
    response
        .clone()
        .on_hover_text("Clic: mostra l'area di stage");
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

/// La riga (della vista) da portare in vista, se non è già tutta visibile.
fn scroll_target(target: Option<usize>, viewport: &ListViewport) -> Option<usize> {
    let target = target?;
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

/// Pannello a destra: messaggio e metadati del commit selezionato, poi i file cambiati.
///
/// `selected` è il commit della lista: mentre `commit.show` lavora se ne mostrano subito
/// messaggio e metadati, i file arrivano dopo. `open` è il file del diff aperto, evidenziato.
pub(crate) fn details_panel(
    ui: &mut Ui,
    selected: Option<&Commit>,
    shown: Shown<'_>,
    open: Option<&DiffTarget>,
    actions: &mut Vec<Action>,
) {
    let (commit, compared_to) = match &shown {
        Shown::Nothing => {
            ui.centered_and_justified(|ui| ui.weak("Nessun commit selezionato."));
            return;
        }
        Shown::Loaded(details) => (Some(&details.commit), Some(details.compared_to)),
        Shown::Loading(id) | Shown::Failed(id, _) => (selected.filter(|c| c.id == *id), None),
    };
    // Il messaggio può essere lungo: scorre da solo e lascia ai file almeno metà pannello.
    let max_header = (ui.available_height() * 0.5).max(120.0);
    egui::ScrollArea::vertical()
        .id_salt("dettagli-testa")
        .max_height(max_header)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if let Some(commit) = commit {
                commit_header(ui, commit, compared_to, actions);
            }
        });
    ui.separator();
    match shown {
        Shown::Nothing => {}
        Shown::Loading(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Caricamento dei file cambiati…");
            });
        }
        Shown::Failed(_, error) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button("Riprova").clicked() {
                actions.push(Action::RetryDetails);
            }
        }
        Shown::Loaded(details) => {
            changed_files(ui, details.commit.id, &details.files, open, actions);
        }
    }
}

/// Oggetto, corpo e metadati. `compared_to` è `None` finché `commit.show` non risponde.
fn commit_header(
    ui: &mut Ui,
    commit: &Commit,
    compared_to: Option<Option<Oid>>,
    actions: &mut Vec<Action>,
) {
    ui.add_space(4.0);
    ui.add(egui::Label::new(RichText::new(&commit.summary).heading()).wrap());
    if !commit.body.is_empty() {
        ui.add_space(4.0);
        ui.add(egui::Label::new(&commit.body).wrap());
    }
    ui.add_space(8.0);
    egui::Grid::new("dettagli-commit")
        .num_columns(2)
        .spacing(vec2(12.0, 4.0))
        .show(ui, |ui| {
            ui.weak("Commit");
            ui.horizontal_wrapped(|ui| {
                let id = commit.id.to_string();
                ui.add(egui::Label::new(RichText::new(&id).monospace()).selectable(true));
                if ui
                    .small_button("Copia")
                    .on_hover_text("Copia l'id completo")
                    .clicked()
                {
                    ui.ctx().copy_text(id);
                }
            });
            ui.end_row();

            ui.weak(if commit.parents.len() == 1 {
                "Genitore"
            } else {
                "Genitori"
            });
            ui.horizontal_wrapped(|ui| {
                if commit.parents.is_empty() {
                    ui.weak("nessuno");
                }
                for parent in &commit.parents {
                    oid_link(ui, *parent, actions);
                }
            });
            ui.end_row();

            for (title, sig) in [("Autore", &commit.author), ("Committer", &commit.committer)] {
                ui.weak(title);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(signer(sig)).wrap());
                    ui.weak(date_time_zone(sig));
                });
                ui.end_row();
            }

            ui.weak("Confrontato con");
            match compared_to {
                None => {
                    ui.weak("…");
                }
                Some(None) => {
                    ui.label("commit radice (albero vuoto)");
                }
                Some(Some(base)) => {
                    ui.horizontal_wrapped(|ui| {
                        oid_link(ui, base, actions);
                        if commit.is_merge() {
                            ui.weak("(primo genitore)");
                        }
                    });
                }
            }
            ui.end_row();
        });
    if commit.is_merge() {
        ui.add_space(4.0);
        ui.colored_label(Color32::from_rgb(0x9b, 0x6b, 0xdf), "Commit di merge");
    }
    ui.add_space(4.0);
}

/// Id breve cliccabile: seleziona il commit nella lista.
fn oid_link(ui: &mut Ui, id: Oid, actions: &mut Vec<Action>) {
    let response = ui
        .link(RichText::new(id.short()).monospace())
        .on_hover_text(format!("{id}\nClic: seleziona il commit"));
    if response.clicked() {
        actions.push(Action::SelectCommit(id));
    }
}

/// Altezza di una riga della lista dei file.
const FILE_ROW_HEIGHT: f32 = 20.0;

/// Colore della lettera del tipo di modifica.
fn change_color(change: Change) -> Color32 {
    match change {
        Change::Added => Color32::from_rgb(0x3f, 0xb9, 0x50),
        Change::Deleted => Color32::from_rgb(0xe5, 0x53, 0x4b),
        Change::Modified => Color32::from_rgb(0xd2, 0x99, 0x22),
        Change::TypeChanged => Color32::from_rgb(0xb0, 0x7c, 0xe8),
        Change::Renamed => Color32::from_rgb(0x4c, 0x9a, 0xf0),
        Change::Copied => Color32::from_rgb(0x2a, 0xb7, 0xa9),
        Change::Unmerged => Color32::from_rgb(0xf5, 0x7c, 0x00),
        _ => Color32::GRAY,
    }
}

/// Elenco dei file cambiati del commit `commit`, virtualizzato: si disegnano solo le righe
/// visibili. Un clic su un file ne apre il diff; il file del diff aperto (`open`) è evidenziato.
fn changed_files(
    ui: &mut Ui,
    commit: Oid,
    files: &[ChangedFile],
    open: Option<&DiffTarget>,
    actions: &mut Vec<Action>,
) {
    ui.strong(match files.len() {
        0 => "Nessun file cambiato".to_owned(),
        1 => "1 file cambiato".to_owned(),
        n => format!("{n} file cambiati"),
    });
    ui.add_space(2.0);
    let font = FontId::proportional(13.0);
    let mono = FontId::monospace(12.0);
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    egui::ScrollArea::vertical()
        .id_salt("dettagli-file")
        .auto_shrink(false)
        .show_rows(ui, FILE_ROW_HEIGHT, files.len(), |ui, range| {
            for file in &files[range] {
                let (rect, response) = ui.allocate_exact_size(
                    vec2(ui.available_width(), FILE_ROW_HEIGHT),
                    Sense::click(),
                );
                let target = DiffTarget::commit(commit, file.path.clone());
                if open == Some(&target) {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                } else if response.hovered() {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let painter = ui.painter_at(rect);
                let color = change_color(file.change);
                let badge = egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 11.0, rect.center().y),
                    vec2(18.0, 16.0),
                );
                painter.rect_filled(badge, 3.0, color);
                painter.text(
                    badge.center(),
                    Align2::CENTER_CENTER,
                    change_letter(file.change),
                    mono.clone(),
                    Color32::from_gray(20),
                );

                // A destra: somiglianza e segno del sottomodulo.
                let mut right = rect.right() - 4.0;
                let mut tail = file_score(file);
                if file.is_submodule() {
                    tail = format!("{tail} [sottomodulo]").trim_start().to_owned();
                }
                if !tail.is_empty() {
                    let galley = painter.layout_no_wrap(tail, font.clone(), weak);
                    right -= galley.size().x;
                    let top = rect.center().y - galley.size().y / 2.0;
                    painter.galley(egui::pos2(right, top), galley, weak);
                    right -= 8.0;
                }

                let left = badge.right() + 6.0;
                let galley = painter.layout_job(path_job(file, &font, text, weak, right - left));
                let top = rect.center().y - galley.size().y / 2.0;
                painter.galley(egui::pos2(left, top), galley, text);

                let response = response.on_hover_text(file_tooltip(file));
                if response.clicked() {
                    actions.push(Action::OpenDiff(target));
                }
            }
        });
}

/// Percorso su una riga, tagliato con `…` se non entra in `width`. Per rinomine e copie
/// l'origine è più tenue.
fn path_job(
    file: &ChangedFile,
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
    if let Some(original) = &file.original {
        job.append(&format!("{original} -> "), 0.0, format(weak));
    }
    job.append(&file.path.to_string(), 0.0, format(text));
    job.wrap.max_width = width.max(10.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    job
}

fn file_tooltip(file: &ChangedFile) -> String {
    let mut lines = vec![format!(
        "{} ({})",
        file_path(file),
        format::change_name(file.change)
    )];
    if let Some(score) = file.score {
        lines.push(format!("somiglianza {score}%"));
    }
    if file.old_mode != file.new_mode && file.old_mode != 0 && file.new_mode != 0 {
        lines.push(format!("modo {:o} -> {:o}", file.old_mode, file.new_mode));
    }
    if file.is_submodule() {
        lines.push("sottomodulo".to_owned());
    }
    lines.push("Clic: mostra il diff".to_owned());
    lines.join("\n")
}

/// Area di stage, al posto dei dettagli quando è selezionata la riga delle modifiche: le liste
/// "In stage" e "Non in stage", ciascuna virtualizzata. `writing` è vero mentre una scrittura
/// è in corso: le azioni che modificano il repo sono disabilitate.
pub(crate) fn stage_panel(ui: &mut Ui, repo: &RepoView, writing: bool, actions: &mut Vec<Action>) {
    ui.add_space(4.0);
    ui.heading("Modifiche non committate");
    if writing {
        ui.horizontal(|ui| {
            ui.spinner();
            let label = repo.write_label.as_deref().unwrap_or("Scrittura");
            ui.weak(format!("{label} in corso…"));
        });
    }
    if let Some(error) = &repo.write_error {
        ui.horizontal_top(|ui| {
            if ui.small_button("x").on_hover_text("Nascondi").clicked() {
                actions.push(Action::DismissWriteError);
            }
            ui.add(
                egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap(),
            );
        });
    }
    ui.separator();
    let enabled = !writing && repo.confirm.is_none();
    let open = repo.diff.target();
    // Le due liste si dividono lo spazio; una lista corta lascia il resto all'altra.
    let section_header = 28.0;
    let spare = (ui.available_height() - 2.0 * section_header - 12.0).max(80.0);
    let staged_h = list_height(repo.stage.staged.len())
        .min((spare / 2.0).max(spare - list_height(repo.stage.unstaged.len())));
    let sides = [
        (Side::Staged, &repo.stage.staged, staged_h),
        (Side::Unstaged, &repo.stage.unstaged, f32::INFINITY),
    ];
    for (i, (side, items, height)) in sides.into_iter().enumerate() {
        if i > 0 {
            ui.separator();
        }
        let height = height.min(ui.available_height() - section_header).max(40.0);
        stage_section(ui, side, items, open, enabled, height, actions);
    }
}

/// Altezza che serve a una lista di `rows` file, almeno una riga.
fn list_height(rows: usize) -> f32 {
    rows.max(1) as f32 * (FILE_ROW_HEIGHT + 2.0)
}

/// Titolo con il conteggio, pulsante per tutta la lista, poi i file.
fn stage_section(
    ui: &mut Ui,
    side: Side,
    items: &[StageItem],
    open: Option<&DiffTarget>,
    enabled: bool,
    height: f32,
    actions: &mut Vec<Action>,
) {
    let (title, op, all_label, all_help) = match side {
        Side::Staged => (
            "In stage",
            StageOp::Unstage,
            "Unstage di tutto",
            "Toglie dallo stage tutti i file di questa lista; il worktree non cambia",
        ),
        Side::Unstaged => (
            "Non in stage",
            StageOp::Stage,
            "Stage di tutto",
            "Mette in stage tutti i file di questa lista",
        ),
    };
    ui.horizontal(|ui| {
        ui.strong(format!("{title} ({})", items.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let button = ui
                .add_enabled(
                    enabled && !items.is_empty(),
                    egui::Button::new(all_label).small(),
                )
                .on_hover_text(all_help);
            if button.clicked()
                && let Some(paths) = stage::all_paths(items)
            {
                actions.push(Action::Write(op, paths));
            }
        });
    });
    if items.is_empty() {
        let text = match side {
            Side::Staged => "Nessun file in stage.",
            Side::Unstaged => "Nessuna modifica fuori dallo stage.",
        };
        ui.weak(text);
        return;
    }
    let source = match side {
        Side::Staged => DiffSource::Index,
        Side::Unstaged => DiffSource::Worktree,
    };
    let font = FontId::proportional(13.0);
    let mono = FontId::monospace(12.0);
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    let button_label = match op {
        StageOp::Stage => "Stage",
        StageOp::Unstage => "Unstage",
    };
    egui::ScrollArea::vertical()
        .id_salt(("area-stage", side == Side::Staged))
        .auto_shrink([false, true])
        .max_height(height)
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
                if open == Some(&target) {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                } else if response.hovered() {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let painter = ui.painter_at(rect);
                let badge = egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 11.0, rect.center().y),
                    vec2(18.0, 16.0),
                );
                painter.rect_filled(badge, 3.0, change_color(item.change));
                painter.text(
                    badge.center(),
                    Align2::CENTER_CENTER,
                    change_letter(item.change),
                    mono.clone(),
                    Color32::from_gray(20),
                );

                // A destra il pulsante della riga.
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
                let button = cell
                    .add_enabled(enabled, egui::Button::new(button_label).small())
                    .on_hover_text(row_action_help(op, item));

                let left = badge.right() + 6.0;
                let right = button_rect.left() - 4.0;
                let job = stage_path_job(item, &font, text, weak, right - left);
                let galley = painter.layout_job(job);
                let top = rect.center().y - galley.size().y / 2.0;
                painter.galley(egui::pos2(left, top), galley, text);

                let response = response.on_hover_text(stage_tooltip(op, item));
                if (button.clicked() || response.double_clicked()) && enabled {
                    actions.push(Action::Write(op, vec![item.path.clone()]));
                } else if response.clicked() {
                    actions.push(Action::OpenDiff(target));
                }
            }
        });
}

fn stage_path_job(
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

fn conflict_name(conflict: Conflict) -> &'static str {
    match conflict {
        Conflict::BothDeleted => "eliminato da entrambi",
        Conflict::AddedByUs => "aggiunto da noi",
        Conflict::DeletedByThem => "eliminato da loro",
        Conflict::AddedByThem => "aggiunto da loro",
        Conflict::DeletedByUs => "eliminato da noi",
        Conflict::BothAdded => "aggiunto da entrambi",
        Conflict::BothModified => "modificato da entrambi",
    }
}

fn row_action_help(op: StageOp, item: &StageItem) -> &'static str {
    match op {
        StageOp::Stage if item.is_conflict() => "Segna il conflitto come risolto (chiede conferma)",
        StageOp::Stage => "Metti in stage questo file",
        StageOp::Unstage => "Togli dallo stage questo file; il worktree non cambia",
    }
}

fn stage_tooltip(op: StageOp, item: &StageItem) -> String {
    let mut lines = Vec::new();
    let path = match &item.original {
        Some(original) => format!("{original} -> {}", item.path),
        None => item.path.to_string(),
    };
    match item.conflict {
        Some(conflict) => lines.push(format!(
            "{path} (in conflitto: {})",
            conflict_name(conflict)
        )),
        None => lines.push(format!("{path} ({})", format::change_name(item.change))),
    }
    lines.push("Clic: mostra il diff".to_owned());
    lines.push(
        match op {
            StageOp::Stage if item.is_conflict() => {
                "Doppio clic: segna il conflitto come risolto (chiede conferma)"
            }
            StageOp::Stage => "Doppio clic: metti in stage",
            StageOp::Unstage => "Doppio clic: togli dallo stage",
        }
        .to_owned(),
    );
    lines.join("\n")
}

/// Quanti file in conflitto elencare nella conferma; gli altri diventano "e altri N".
const CONFIRM_LIST: usize = 8;

/// Conferma prima di mettere in stage file in conflitto: spiega che vengono segnati risolti.
pub(crate) fn confirm_write(ctx: &egui::Context, write: &PendingWrite, actions: &mut Vec<Action>) {
    let modal = egui::Modal::new(egui::Id::unique("conferma-scrittura")).show(ctx, |ui| {
        ui.set_max_width(440.0);
        let n = write.conflicts.len();
        ui.heading(if n == 1 {
            "Segnare il conflitto come risolto?"
        } else {
            "Segnare i conflitti come risolti?"
        });
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                "Mettere in stage un file in conflitto lo segna come risolto: nell'indice va il \
                 contenuto attuale del file nel worktree, così com'è, compresi eventuali \
                 marcatori di conflitto (<<<<<<<, =======, >>>>>>>) rimasti. Le versioni del \
                 conflitto (base, nostra e loro) escono dall'indice. Controllare prima che il \
                 file sia davvero risolto.",
            )
            .wrap(),
        );
        ui.add_space(6.0);
        ui.strong(if n == 1 {
            "File in conflitto:".to_owned()
        } else {
            format!("{n} file in conflitto:")
        });
        for path in write.conflicts.iter().take(CONFIRM_LIST) {
            ui.monospace(path.to_string_lossy());
        }
        if n > CONFIRM_LIST {
            ui.weak(format!("e altri {}", n - CONFIRM_LIST));
        }
        let others = write.paths.len() - n;
        if others > 0 {
            ui.add_space(4.0);
            ui.weak(format!(
                "Insieme a {others} altri file senza conflitti, messi in stage normalmente."
            ));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let label = if n == 1 {
                "Segna come risolto"
            } else {
                "Segna come risolti"
            };
            if ui.button(label).clicked() {
                actions.push(Action::ConfirmWrite);
            }
            if ui.button("Annulla").clicked() {
                actions.push(Action::CancelWrite);
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::CancelWrite);
    }
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
