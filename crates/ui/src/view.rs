//! Disegno dei pannelli. Le funzioni leggono la vista e producono [`Action`]; non cambiano
//! niente direttamente.

use crate::branch::{self, Dialog, NameKind, Start};
use crate::commit::{self, CommitDraft, ConfirmAmend, Gate};
use crate::details::Shown;
use crate::diff::{DiffSource, DiffTarget};
use crate::discard::{self, Discard, UndoOp};
use crate::fetch::{self, Failure, Running};
use crate::format::{self, change_letter, date_time, date_time_zone, file_path, file_score};
use crate::graph::{self, LabelKind, RefLabel};
use crate::list::{Anchor, CommitList};
use crate::stage::{self, PendingWrite, Side, StageItem, StageOp};
use crate::{Action, Job, ListViewport, RepoView};
use egui::{Align2, Color32, FontId, RichText, Sense, Ui, vec2};
use project_domain::{
    Change, ChangedFile, Commit, Conflict, Head, Oid, RefInfo, RefKind, Signature, Upstream,
};
use project_engine::Progress;

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

/// `writing`: c'è una scrittura in corso, Annulla, Ripeti, Aggiorna e Fetch aspettano.
/// `fetching`: il fetch in corso con il suo ultimo avanzamento.
pub(crate) fn top_bar(
    ui: &mut Ui,
    repo: Option<&RepoView>,
    can_pick: bool,
    writing: bool,
    fetching: Option<(&Running, Option<Progress>)>,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        if can_pick && ui.button("Apri…").clicked() {
            actions.push(Action::PickFolder);
        }
        let Some(repo) = repo else {
            return;
        };
        let button = ui
            .add_enabled(!writing, egui::Button::new("Aggiorna"))
            .on_hover_text("Rilegge ref, stato e log")
            .on_disabled_hover_text("Dopo l'operazione in corso");
        if button.clicked() {
            actions.push(Action::Refresh);
        }
        ui.separator();
        match fetching {
            Some((running, progress)) => fetch_progress(ui, running, progress.as_ref(), actions),
            None => fetch_buttons(ui, repo, !writing && !repo.modal_open(), actions),
        }
        ui.separator();
        for op in [UndoOp::Undo, UndoOp::Redo] {
            let available = op.available(&repo.history);
            let enabled = available && !writing && !repo.modal_open();
            let button = ui.add_enabled(enabled, egui::Button::new(op.label()));
            let hint = op.hint(&repo.history);
            let button = if available && !enabled {
                button.on_disabled_hover_text(format!("{hint} (dopo l'operazione in corso)"))
            } else {
                button.on_hover_text(&hint).on_disabled_hover_text(&hint)
            };
            if button.clicked() {
                actions.push(Action::Undo(op));
            }
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

/// Il pulsante "Fetch" e, con più remoti, il menu per sceglierne uno.
fn fetch_buttons(ui: &mut Ui, repo: &RepoView, enabled: bool, actions: &mut Vec<Action>) {
    let button = ui
        .add_enabled(enabled, egui::Button::new("Fetch"))
        .on_hover_text(
            "Scarica commit, branch remoti e tag dal remoto dell'upstream del branch attivo \
             (altrimenti origin, o l'unico remoto). Non tocca branch locali, indice né file",
        )
        .on_disabled_hover_text(WAIT);
    if button.clicked() {
        actions.push(Action::Fetch(None));
    }
    let remotes = fetch::remotes(&repo.refs);
    if remotes.len() > 1 {
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button("Da…", |ui| {
                for remote in remotes {
                    if ui.button(format!("Fetch da «{remote}»")).clicked() {
                        actions.push(Action::Fetch(Some(remote)));
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Sceglie il remoto da cui fare il fetch");
        });
    }
}

/// Il fetch in corso: fase, barra (se git dà la percentuale) e "Interrompi".
fn fetch_progress(
    ui: &mut Ui,
    running: &Running,
    progress: Option<&Progress>,
    actions: &mut Vec<Action>,
) {
    ui.spinner();
    let (text, fraction) = match progress {
        Some(p) => fetch::describe(p),
        None => ("Connessione al remoto…".to_owned(), None),
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
        .add_enabled(!running.cancelling, egui::Button::new("Interrompi"))
        .on_hover_text(
            "Ferma il fetch: i branch remoti già aggiornati restano, i branch locali e i file \
             non sono toccati",
        )
        .on_disabled_hover_text("Interruzione in corso…");
    if stop.clicked() {
        actions.push(Action::CancelFetch);
    }
}

/// La finestra di un fetch non riuscito: cosa è successo, cosa fare, il testo di git. Non
/// blocca il resto della finestra.
pub(crate) fn fetch_failure(ctx: &egui::Context, failure: &Failure, actions: &mut Vec<Action>) {
    let mut open = true;
    egui::Window::new(&failure.title)
        .id(egui::Id::unique("errore-fetch"))
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 60.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(&failure.explanation);
            if let Some(git) = failure.git.as_deref().filter(|g| !g.trim().is_empty()) {
                ui.add_space(6.0);
                ui.label("Messaggio di git:");
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(git.trim_end()).monospace()).wrap());
                    });
            }
            ui.add_space(6.0);
            if ui.button("Chiudi").clicked() {
                actions.push(Action::DismissFetchError);
            }
        });
    if !open {
        actions.push(Action::DismissFetchError);
    }
}

/// `write` è la scrittura in corso, descritta: ha la precedenza sugli altri comandi. `notice`
/// è un messaggio informativo, mostrato se non c'è niente in corso né un errore.
pub(crate) fn status_bar(
    ui: &mut Ui,
    running: &[Job],
    write: Option<&str>,
    error: &str,
    notice: &str,
) {
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
        } else if !notice.is_empty() {
            ui.label(notice);
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

/// `writing`: c'è una scrittura in corso, le voci del menu che scrivono aspettano.
pub(crate) fn refs_panel(ui: &mut Ui, repo: &RepoView, writing: bool, actions: &mut Vec<Action>) {
    let current = match &repo.head {
        Head::Branch { name, .. } => Some(format!("refs/heads/{name}")),
        Head::Detached(_) => None,
    };
    let can_write = !writing && !repo.modal_open();
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
                        let is_head = current.as_deref() == Some(&info.name);
                        ref_row(ui, info, is_head, can_write, actions);
                    }
                });
        }
    });
}

/// `can_write`: le voci che scrivono sono attive (nessuna scrittura in corso, nessun dialogo).
fn ref_row(ui: &mut Ui, info: &RefInfo, is_head: bool, can_write: bool, actions: &mut Vec<Action>) {
    // L'id della riga viene dal nome del ref, non dalla posizione: se i ref si rileggono fra
    // i due clic, un doppio clic non finisce su un altro branch.
    ui.push_id(&info.name, |ui| {
        ref_row_inner(ui, info, is_head, can_write, actions);
    });
}

fn ref_row_inner(
    ui: &mut Ui,
    info: &RefInfo,
    is_head: bool,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let local = info.kind == RefKind::LocalBranch;
    ui.horizontal(|ui| {
        // Il branch corrente è evidenziato e in grassetto.
        let name = RichText::new(info.short_name());
        let name = if is_head { name.strong() } else { name };
        let response = ui.selectable_label(is_head, name);
        if response.double_clicked() && local && !is_head && can_write {
            actions.push(Action::Checkout(info.short_name().to_owned()));
        } else if response.clicked() {
            actions.push(Action::SelectCommit(info.target));
        }
        if info.kind != RefKind::Stash {
            response.context_menu(|ui| ref_menu(ui, info, is_head, can_write, actions));
        }
        if let Some(upstream) = &info.upstream {
            let text = ahead_behind(upstream);
            if !text.is_empty() {
                ui.weak(text);
            }
        }
    })
    .response
    .on_hover_text(ref_tooltip(info, is_head));
}

fn ref_tooltip(info: &RefInfo, is_head: bool) -> String {
    let mut text = format!("{}\n{}\nClic: seleziona il commit", info.name, info.target);
    match info.kind {
        RefKind::LocalBranch if is_head => text.push_str("\nClic destro: altre azioni"),
        RefKind::LocalBranch => text.push_str("\nDoppio clic: checkout\nClic destro: altre azioni"),
        RefKind::RemoteBranch | RefKind::Tag => text.push_str("\nClic destro: nuovo branch da qui"),
        RefKind::Stash => {}
    }
    text
}

/// Quando una voce che scrive è spenta perché si aspetta: il motivo.
const WAIT: &str = "Aspettare l'operazione in corso (o chiudere il dialogo aperto)";

/// Il menu contestuale di un ref: sui branch locali checkout, nuovo branch, rinomina ed
/// eliminazione; su branch remoti e tag solo il nuovo branch.
fn ref_menu(
    ui: &mut Ui,
    info: &RefInfo,
    is_head: bool,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let name = info.short_name();
    if info.kind == RefKind::LocalBranch {
        let button = ui
            .add_enabled(can_write && !is_head, egui::Button::new("Checkout"))
            .on_hover_text(format!(
                "Passa a «{name}»: le modifiche locali restano, se non toccano file diversi nel \
                 branch (altrimenti niente cambia)"
            ))
            .on_disabled_hover_text(if is_head {
                "È già il branch attivo"
            } else {
                WAIT
            });
        if button.clicked() {
            actions.push(Action::Checkout(name.to_owned()));
            ui.close();
        }
    }
    let button = ui
        .add_enabled(can_write, egui::Button::new("Nuovo branch da qui…"))
        .on_hover_text(format!(
            "Crea un branch su {} senza passarci",
            info.target.short()
        ))
        .on_disabled_hover_text(WAIT);
    if button.clicked() {
        let (start, proposed) = Start::from_ref(info);
        actions.push(Action::NewBranch(start, proposed));
        ui.close();
    }
    if info.kind != RefKind::LocalBranch {
        return;
    }
    let button = ui
        .add_enabled(can_write, egui::Button::new("Rinomina…"))
        .on_disabled_hover_text(WAIT);
    if button.clicked() {
        actions.push(Action::RenameBranch(name.to_owned()));
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
            egui::Button::new(RichText::new("Elimina").color(color)),
        )
        .on_hover_text(
            "Elimina il branch. Se è unito si elimina subito; se ha commit che non sono altrove \
             chiede conferma. In ogni caso il commit resta in un backup \
             (refs/project/backup/delete-…) e «Annulla» ricrea il branch",
        )
        .on_disabled_hover_text(if is_head {
            "È il branch attivo: passare prima a un altro branch per eliminarlo"
        } else {
            WAIT
        });
    if button.clicked() {
        actions.push(Action::DeleteBranch(name.to_owned(), info.target));
        ui.close();
    }
}

/// Altezza di una riga della lista dei commit.
const ROW_HEIGHT: f32 = 22.0;

/// `writing`: c'è una scrittura in corso, le voci del menu che scrivono aspettano.
pub(crate) fn commit_list(
    ui: &mut Ui,
    repo: &RepoView,
    viewport: &mut ListViewport,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    let can_write = !writing && !repo.modal_open();
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

    // La riga delle modifiche sta nella colonna di HEAD, che non è sempre il primo commit
    // (un branch remoto appena scaricato può essere più recente).
    let anchor = repo.head.target().and_then(|head| list.anchor(head));
    let output = area.show_rows(ui, ROW_HEIGHT, rows, |ui, range| {
        // Righe di commit disegnate: il grafo va sopra gli sfondi di tutte, quindi dopo.
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
                    Some(id) => format!("Ricerca del commit {} nei commit più vecchi…", id.short()),
                    None => "Caricamento di altri commit…".to_owned(),
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
                    .add_enabled(can_write, egui::Button::new("Nuovo branch da qui…"))
                    .on_hover_text(format!("Crea un branch su {} senza passarci", id.short()))
                    .on_disabled_hover_text(WAIT);
                if button.clicked() {
                    actions.push(Action::NewBranch(Start::from_commit(id), String::new()));
                    ui.close();
                }
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

/// Il tratto dall'anello delle modifiche giù fino al pallino di HEAD, se la colonna di HEAD è
/// libera nelle righe in mezzo (altrimenti coprirebbe gli archi di altri commit).
fn paint_changes_link(
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

/// La riga "Modifiche non committate": un anello vuoto nel grafo (nella colonna `column` di
/// HEAD), il titolo e i conteggi.
fn changes_row(
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
/// "In stage" e "Non in stage", ciascuna virtualizzata, e sotto la casella del messaggio di
/// commit. `writing` è vero mentre una scrittura è in corso: le azioni che modificano il repo
/// sono disabilitate. `draft` è la bozza del messaggio, che i campi di testo cambiano.
pub(crate) fn stage_panel(
    ui: &mut Ui,
    repo: &RepoView,
    draft: &mut CommitDraft,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    egui::Panel::bottom("messaggio-commit")
        .resizable(false)
        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 6)))
        .show(ui, |ui| commit_box(ui, repo, draft, writing, actions));
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
    let enabled = !writing && !repo.modal_open();
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

/// Righe del corpo del messaggio.
const BODY_ROWS: usize = 4;
/// Altezza massima dell'output di git dopo un commit fallito; oltre si scorre.
const OUTPUT_HEIGHT: f32 = 140.0;

/// Casella del messaggio di commit: oggetto con il contatore, corpo, amend, pulsante "Commit"
/// (anche Ctrl+Invio) e l'errore dell'ultimo commit.
fn commit_box(
    ui: &mut Ui,
    repo: &RepoView,
    draft: &mut CommitDraft,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    let sending = repo.commit.sending().is_some();
    let gate = Gate {
        draft,
        ..repo.commit_gate(writing)
    }
    .check();
    // Prima dei campi di testo, che altrimenti riceverebbero il tasto.
    let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Enter);
    if !repo.modal_open() && ui.input_mut(|i| i.consume_shortcut(&shortcut)) && gate.is_ok() {
        actions.push(Action::Commit);
    }

    ui.horizontal(|ui| {
        ui.strong("Messaggio di commit");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let len = draft.subject_len();
            let text = format!("{len}/{}", commit::SUBJECT_LIMIT);
            if draft.subject_too_long() {
                ui.label(RichText::new(text).small().color(ui.visuals().warn_fg_color))
                    .on_hover_text(format!(
                        "L'oggetto supera {} caratteri: nel log e in molti strumenti viene                          tagliato. Il resto può andare nel corpo.",
                        commit::SUBJECT_LIMIT
                    ));
            } else {
                ui.label(RichText::new(text).small().weak())
                    .on_hover_text("Caratteri dell'oggetto");
            }
        });
    });
    ui.add_enabled(
        !sending,
        egui::TextEdit::singleline(&mut draft.subject)
            .id_salt("oggetto-commit")
            .hint_text("Oggetto (una riga breve)")
            .desired_width(f32::INFINITY),
    );
    ui.add_enabled(
        !sending,
        egui::TextEdit::multiline(&mut draft.body)
            .id_salt("corpo-commit")
            .hint_text("Descrizione (facoltativa)")
            .desired_rows(BODY_ROWS)
            .desired_width(f32::INFINITY),
    );

    let mut amend = draft.amend();
    let toggle = ui
        .add_enabled(
            !sending && !writing,
            egui::Checkbox::new(&mut amend, "Amend (modifica l'ultimo commit)"),
        )
        .on_hover_text(
            "Sostituisce l'ultimo commit con uno nuovo: stesso autore e genitori, il messaggio              scritto qui e le modifiche in stage aggiunte. Il commit sostituito resta salvato in              un ref di backup.",
        );
    if toggle.changed() {
        actions.push(Action::SetAmend(amend));
    }
    if draft.amend() {
        let what = match repo.head_commit() {
            Some(c) => format!("l'ultimo commit {} «{}»", c.id.short(), c.summary),
            None => match repo.head.target() {
                Some(id) => format!("l'ultimo commit {}", id.short()),
                None => "l'ultimo commit".to_owned(),
            },
        };
        let mut text = format!("Cambierà {what}: verrà sostituito da un commit nuovo.");
        if draft.is_proposed() {
            text.push_str(" Il messaggio è quello attuale: modificarlo se serve.");
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
            "Commit (amend)"
        } else {
            "Commit"
        };
        let button = ui.add_enabled(gate.is_ok(), egui::Button::new(label));
        let button = match gate {
            Ok(()) => button.on_hover_text("Crea il commit (Ctrl+Invio)"),
            Err(blocked) => button.on_disabled_hover_text(blocked.hint()),
        };
        if button.clicked() {
            actions.push(Action::Commit);
        }
        if sending {
            ui.spinner();
            let label = repo.write_label.as_deref().unwrap_or("Commit");
            ui.weak(format!("{label} in corso…"));
        } else {
            ui.weak("Ctrl+Invio");
        }
    });
    // Perché il pulsante è spento, senza dover passare il mouse (non durante il lavoro).
    if let Err(blocked) = gate
        && !matches!(blocked, commit::Blocked::Busy)
    {
        ui.add(egui::Label::new(RichText::new(blocked.hint()).small().weak()).wrap());
    }

    if let Some(error) = repo.commit.error() {
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            if ui.small_button("x").on_hover_text("Nascondi").clicked() {
                actions.push(Action::DismissCommitError);
            }
            ui.add(
                egui::Label::new(RichText::new(&error.message).color(ui.visuals().error_fg_color))
                    .wrap(),
            );
        });
        if let Some(output) = &error.output {
            egui::ScrollArea::vertical()
                .id_salt("output-commit")
                .max_height(OUTPUT_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    // Un `&str` rende il campo di sola lettura, ma selezionabile e copiabile.
                    ui.add(
                        egui::TextEdit::multiline(&mut output.as_str())
                            .id_salt("output-commit-testo")
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(1)
                            .desired_width(f32::INFINITY),
                    );
                });
            if ui
                .small_button("Copia l'output")
                .on_hover_text("Copia negli appunti tutto l'output di git")
                .clicked()
            {
                ui.ctx().copy_text(output.clone());
            }
        }
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
            if side == Side::Unstaged {
                let paths = discard::discardable_paths(items);
                let button = ui
                    .add_enabled(
                        enabled && paths.is_some(),
                        egui::Button::new("Scarta tutto…").small(),
                    )
                    .on_hover_text(
                        "Scarta le modifiche fuori stage di tutti i file tracciati di questa \
                         lista (chiede conferma, si può annullare). I file non tracciati non si \
                         toccano",
                    );
                if button.clicked()
                    && let Some(paths) = paths
                {
                    actions.push(Action::Discard(paths));
                }
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
                    actions.push(Action::OpenDiff(target.clone()));
                }
                response.context_menu(|ui| {
                    file_menu(ui, op, item, &target, enabled, actions);
                });
            }
        });
}

/// Il menu contestuale di un file dell'area di stage: diff, stage/unstage e, fuori stage,
/// "Scarta modifiche…".
fn file_menu(
    ui: &mut Ui,
    op: StageOp,
    item: &StageItem,
    target: &DiffTarget,
    enabled: bool,
    actions: &mut Vec<Action>,
) {
    if ui.button("Mostra il diff").clicked() {
        actions.push(Action::OpenDiff(target.clone()));
        ui.close();
    }
    let label = match op {
        StageOp::Stage if item.is_conflict() => "Segna come risolto…",
        StageOp::Stage => "Stage",
        StageOp::Unstage => "Unstage",
    };
    let button = ui
        .add_enabled(enabled, egui::Button::new(label))
        .on_hover_text(row_action_help(op, item));
    if button.clicked() {
        actions.push(Action::Write(op, vec![item.path.clone()]));
        ui.close();
    }
    if op == StageOp::Stage {
        ui.separator();
        let reason = discard::not_discardable_reason(item);
        let button = ui
            .add_enabled(
                enabled && reason.is_none(),
                egui::Button::new(RichText::new("Scarta modifiche…").color(
                    if reason.is_none() && enabled {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().weak_text_color()
                    },
                )),
            )
            .on_hover_text(
                "Riporta il file alla versione in stage (o dell'ultimo commit): chiede conferma, \
                 si può annullare",
            )
            .on_disabled_hover_text(reason.unwrap_or("Aspettare l'operazione in corso"));
        if button.clicked() {
            actions.push(Action::Discard(vec![item.path.clone()]));
            ui.close();
        }
    }
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
    if op == StageOp::Stage && discard::discardable(item) {
        lines.push("Clic destro: altre azioni, anche scartare le modifiche".to_owned());
    } else {
        lines.push("Clic destro: altre azioni".to_owned());
    }
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

/// Altezza massima dell'elenco dei file nella conferma dello scarto; oltre si scorre.
const DISCARD_LIST_HEIGHT: f32 = 160.0;

/// Conferma prima di scartare le modifiche fuori stage: elenca i file e spiega come si
/// annulla.
pub(crate) fn confirm_discard(ctx: &egui::Context, pending: &Discard, actions: &mut Vec<Action>) {
    let modal = egui::Modal::new(egui::Id::unique("conferma-scarto")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        let n = pending.paths.len();
        ui.heading("Scartare le modifiche?");
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                "Le modifiche non in stage vengono tolte dal worktree: ogni file torna come è \
                 nell'indice, cioè alla versione in stage, o a quella dell'ultimo commit se non \
                 ha niente in stage. Quello che è in stage resta com'è; i file non tracciati non \
                 vengono toccati.",
            )
            .wrap(),
        );
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                "Prima di scartare si salva un backup (refs/project/backup/discard-…): con \
                 «Annulla» nella barra in alto i file tornano come sono adesso, finché non \
                 vengono modificati di nuovo.",
            )
            .wrap(),
        );
        ui.add_space(6.0);
        ui.strong(if n == 1 {
            "File:".to_owned()
        } else {
            format!("{n} file:")
        });
        let row = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::vertical()
            .id_salt("scarto-file")
            .max_height(DISCARD_LIST_HEIGHT)
            .auto_shrink([false, true])
            .show_rows(ui, row, n, |ui, range| {
                for path in &pending.paths[range] {
                    ui.monospace(path.to_string_lossy());
                }
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let label = if n == 1 {
                "Scarta le modifiche"
            } else {
                "Scarta le modifiche di tutti"
            };
            let button =
                egui::Button::new(RichText::new(label).color(ui.visuals().strong_text_color()))
                    .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
            if ui.add(button).clicked() {
                actions.push(Action::ConfirmDiscard);
            }
            if ui.button("Non scartare").clicked() {
                actions.push(Action::CancelDiscard);
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::CancelDiscard);
    }
}

/// Conferma prima di riscrivere con amend un commit già su un branch remoto.
pub(crate) fn confirm_amend(
    ctx: &egui::Context,
    confirm: &ConfirmAmend,
    left_behind: &[String],
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("conferma-amend")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading("Riscrivere un commit già pubblicato?");
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                "L'ultimo commit è già pubblicato o etichettato: lo contiene un branch remoto o \
                 un tag. Con amend viene sostituito da un commit nuovo: i branch remoti e i tag \
                 restano sul commit vecchio, il branch locale diverge da quello remoto e per \
                 pubblicare la modifica servirà un push forzato, che riscrive la storia anche per \
                 chi ha già scaricato il commit. Il commit attuale resta salvato in un ref di \
                 backup.",
            )
            .wrap(),
        );
        ui.add_space(6.0);
        ui.strong("Motivo:");
        ui.add(egui::Label::new(RichText::new(&confirm.reason).monospace()).wrap());
        if !left_behind.is_empty() {
            ui.add_space(6.0);
            ui.strong("Restano sul commit vecchio anche:");
            for name in left_behind.iter().take(CONFIRM_LIST) {
                ui.monospace(name);
            }
            if left_behind.len() > CONFIRM_LIST {
                ui.weak(format!("e altri {}", left_behind.len() - CONFIRM_LIST));
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Riscrivi il commit").clicked() {
                actions.push(Action::ConfirmCommit);
            }
            if ui.button("Annulla").clicked() {
                actions.push(Action::CancelCommit);
            }
        });
    });
    if modal.should_close() {
        actions.push(Action::CancelCommit);
    }
}

/// Il dialogo dei branch aperto: nome (nuovo o rinomina), conferma dell'eliminazione,
/// checkout rifiutato.
pub(crate) fn branch_dialog(ctx: &egui::Context, dialog: &Dialog, actions: &mut Vec<Action>) {
    let modal = egui::Modal::new(egui::Id::unique("dialogo-branch")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        match dialog {
            Dialog::Name(dialog) => name_dialog(ui, dialog, actions),
            Dialog::Delete(delete) => {
                let discard::Phase::Confirm { reason } = &delete.phase else {
                    return;
                };
                delete_dialog(ui, delete, reason, actions);
            }
            Dialog::Refused(refused) => refused_dialog(ui, refused, actions),
        }
    });
    if modal.should_close() {
        actions.push(Action::CloseBranchDialog);
    }
}

fn name_dialog(ui: &mut Ui, dialog: &branch::NameDialog, actions: &mut Vec<Action>) {
    let (title, button) = match &dialog.kind {
        NameKind::Create(_) => ("Nuovo branch", "Crea il branch"),
        NameKind::Rename(_) => ("Rinomina il branch", "Rinomina"),
    };
    ui.heading(title);
    ui.add_space(6.0);
    match &dialog.kind {
        NameKind::Create(start) => {
            ui.label(format!(
                "Il branch parte da {} ({}). Si crea senza passarci: il branch attivo non cambia.",
                start.label,
                start.target.short()
            ));
        }
        NameKind::Rename(old) => {
            ui.label(format!(
                "Nuovo nome per «{old}». Reflog e upstream seguono il branch."
            ));
        }
    }
    ui.add_space(6.0);
    let id = ui.make_persistent_id("nome-branch");
    // Invio nel campo manda il nome: si legge prima del campo, che altrimenti lo prenderebbe.
    let enter = ui.memory(|m| m.has_focus(id))
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let mut name = dialog.name.clone();
    let edit = egui::TextEdit::singleline(&mut name)
        .id(id)
        .hint_text("es. feature/nuova")
        .desired_width(f32::INFINITY)
        .interactive(!dialog.sending);
    let response = ui.add(edit);
    // Il campo prende la tastiera all'apertura.
    if !dialog.sending && ui.memory(|m| m.focused().is_none()) {
        response.request_focus();
    }
    if response.changed() {
        actions.push(Action::BranchName(name));
    }
    if let Some(error) = &dialog.error {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap());
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let clicked = ui
            .add_enabled(dialog.can_submit(), egui::Button::new(button))
            .clicked();
        if (clicked || enter) && dialog.can_submit() {
            actions.push(Action::SubmitBranchName);
        }
        if ui
            .add_enabled(!dialog.sending, egui::Button::new("Chiudi"))
            .clicked()
        {
            actions.push(Action::CloseBranchDialog);
        }
        if dialog.sending {
            ui.spinner();
        }
    });
}

fn delete_dialog(
    ui: &mut Ui,
    delete: &branch::DeleteDialog,
    reason: &str,
    actions: &mut Vec<Action>,
) {
    ui.heading(format!(
        "Eliminare «{}» ({})?",
        delete.name,
        delete.target.short()
    ));
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Il branch non è unito: ha commit che non sono nel branch attivo (né nel suo \
             upstream). Eliminandolo quei commit non sono più raggiungibili da nessun branch.",
        )
        .wrap(),
    );
    ui.add_space(6.0);
    ui.strong("Motivo:");
    ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Prima di eliminare si salva un backup del commit (refs/project/backup/delete-…): \
             con «Annulla» nella barra in alto il branch torna sullo stesso commit, con la \
             sua configurazione (upstream compreso), finché l'app resta aperta: il diario delle \
             operazioni è in memoria. Dopo la chiusura resta solo il backup del commit in \
             refs/project/backup/, da cui il branch si ricrea a mano.",
        )
        .wrap(),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = egui::Button::new(
            RichText::new("Elimina il branch").color(ui.visuals().strong_text_color()),
        )
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
        if ui.add(button).clicked() {
            actions.push(Action::ConfirmDeleteBranch);
        }
        if ui.button("Non eliminare").clicked() {
            actions.push(Action::CloseBranchDialog);
        }
    });
}

/// Altezza massima dell'elenco dei file del checkout rifiutato; oltre si scorre.
const REFUSED_LIST_HEIGHT: f32 = 160.0;

fn refused_dialog(ui: &mut Ui, refused: &branch::Refused, actions: &mut Vec<Action>) {
    ui.heading(format!("Checkout di «{}» non possibile", refused.name));
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Il checkout sovrascriverebbe modifiche locali: questi percorsi sono modificati (in \
             stage o no) o non tracciati (anche dentro una cartella che nel branch è un file, o \
             in un submodule), e nel branch di destinazione sono diversi. Niente è stato \
             modificato. Committare, scartare o mettere da parte queste modifiche e \
             riprovare.",
        )
        .wrap(),
    );
    ui.add_space(6.0);
    let n = refused.files.len();
    ui.strong(match n {
        0 => "Git non ha indicato i file.".to_owned(),
        1 => "File:".to_owned(),
        n => format!("{n} file:"),
    });
    let row = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::vertical()
        .id_salt("checkout-file")
        .max_height(REFUSED_LIST_HEIGHT)
        .auto_shrink([false, true])
        .show_rows(ui, row, n, |ui, range| {
            for path in &refused.files[range] {
                ui.monospace(path);
            }
        });
    ui.add_space(10.0);
    if ui.button("Chiudi").clicked() {
        actions.push(Action::CloseBranchDialog);
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
