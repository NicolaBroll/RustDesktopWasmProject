//! Disegno della vista del diff, al posto del grafo (`impl-docs/ARCHITETTURA-git.md` §8).
//! Legge lo stato di [`crate::diff`] e produce [`Action`]; non cambia niente direttamente.

use crate::diff::{
    DiffDoc, DiffSource, DiffTarget, Pane, Row, display_line, file_summary, file_title,
    line_number, line_prefix,
};
use crate::lines::{self, LineButtons, LineSelection, Partial};
use crate::{Action, Drag};
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, pos2, vec2};
use project_domain::LineKind;

/// Altezza di una riga del diff: tutte uguali, per `show_rows`.
const ROW_HEIGHT: f32 = 18.0;
/// Margine prima del testo e tra le colonne.
const PAD: f32 = 6.0;

/// Cosa serve alla vista per la selezione di righe.
#[derive(Clone, Copy)]
pub(crate) struct LinesUi<'a> {
    /// Le righe selezionate del diff aperto.
    pub selection: Option<&'a LineSelection>,
    /// Falso durante una scrittura o con una conferma aperta.
    pub can_write: bool,
}

/// La selezione parziale per il diff caricato: se si può, e i pulsanti.
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
    toolbar(ui, target, pane, actions);
    if let Some(state) = &state {
        lines_bar(ui, state, actions);
    }
    ui.separator();
    match pane {
        Pane::Closed => {}
        Pane::Loading(target) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(format!(
                    "Caricamento del diff di {}…",
                    target.path.to_string_lossy()
                ));
            });
        }
        Pane::Failed(_, error) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button("Riprova").clicked() {
                actions.push(Action::RetryDiff);
            }
        }
        Pane::Loaded(target, doc) if doc.diff.files.is_empty() => {
            let text = match target.source {
                DiffSource::Worktree => {
                    "Nessuna differenza rispetto all'indice. Un file non tracciato non ha diff \
                     finché non è in stage: dopo lo stage il contenuto compare tra le modifiche \
                     in stage."
                }
                _ => "Nessuna differenza per questo file.",
            };
            ui.centered_and_justified(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(text).weak()).wrap())
            });
        }
        Pane::Loaded(_, doc) => {
            if let Some(state) = &state {
                rows(ui, doc, state, actions);
            }
        }
    }
}

/// Chiudi, percorso e commit, righe aggiunte e tolte.
fn toolbar(ui: &mut Ui, target: &DiffTarget, pane: Pane<'_>, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        if ui
            .button("< Grafo")
            .on_hover_text("Chiudi il diff e torna al grafo (Esc)")
            .clicked()
        {
            actions.push(Action::CloseDiff);
        }
        ui.separator();
        ui.strong(target.path.to_string_lossy());
        match target.source {
            DiffSource::Commit(id) => {
                ui.weak(format!("nel commit {}", id.short()))
                    .on_hover_text(id.to_string());
            }
            DiffSource::Worktree => {
                ui.weak("non in stage")
                    .on_hover_text("Modifiche nel worktree rispetto all'indice");
            }
            DiffSource::Index => {
                ui.weak("in stage")
                    .on_hover_text("Modifiche nell'indice rispetto all'ultimo commit (HEAD)");
            }
        }
        if let Pane::Loaded(_, doc) = pane
            && doc.line_counts() != (0, 0)
        {
            let (added, removed) = doc.line_counts();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(removed_text(ui), format!("-{removed}"));
                ui.colored_label(added_text(ui), format!("+{added}"));
            });
        }
    });
}

/// Sotto la barra del diff: i pulsanti delle righe selezionate, oppure perché il file non si
/// può dividere in righe.
fn lines_bar(ui: &mut Ui, state: &LineState<'_>, actions: &mut Vec<Action>) {
    if let Partial::Unavailable(reason) = &state.partial {
        ui.horizontal(|ui| {
            ui.weak("Stage per hunk e per riga non disponibile")
                .on_hover_text(reason);
            ui.add(egui::Label::new(egui::RichText::new(format!("({reason})")).weak()).truncate())
                .on_hover_text(reason);
        });
        return;
    }
    let Some(b) = state.buttons else {
        return;
    };
    let verb = lines::verb(b.direction);
    ui.horizontal(|ui| {
        let help = match b.direction {
            project_domain::PatchDirection::Stage => {
                "Mette in stage solo le righe selezionate; il resto del file resta fuori"
            }
            project_domain::PatchDirection::Unstage => {
                "Toglie dallo stage solo le righe selezionate; il worktree non cambia"
            }
        };
        let button = ui
            .add_enabled(
                b.selected,
                egui::Button::new(format!("{verb} righe selezionate ({})", b.count)),
            )
            .on_hover_text(help)
            .on_disabled_hover_text(if b.count == 0 {
                "Nessuna riga selezionata: clic su una riga + o - per selezionarla"
            } else {
                "Attendere la fine della scrittura in corso"
            });
        if button.clicked() {
            actions.push(Action::WriteSelectedLines);
        }
        let clear = ui
            .add_enabled(b.clear, egui::Button::new("Deseleziona"))
            .on_hover_text("Deseleziona tutte le righe (Esc)");
        if clear.clicked() {
            actions.push(Action::ClearLines);
        }
        let hint = "Clic su una riga + o - per selezionarla, Maiusc+clic per un intervallo, \
                    trascina per più righe";
        ui.add(egui::Label::new(egui::RichText::new(hint).weak()).truncate())
            .on_hover_text(hint);
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

/// Colori e misure letti una volta per frame.
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
    hunk_bg: Color32,
    hunk_text: Color32,
    file_bg: Color32,
    gutter_bg: Color32,
    /// Tinta delle righe selezionate e colore della barra a sinistra.
    selected_bg: Color32,
    selected_bar: Color32,
    hover_bg: Color32,
    /// Larghezza della colonna dei numeri (due numeri e il segno).
    gutter_w: f32,
    number_w: usize,
}

impl Style {
    fn new(ui: &Ui, doc: &DiffDoc) -> Self {
        let mono = FontId::monospace(12.5);
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
        // `vecchio nuovo +`, con margini.
        let gutter_w = (2 * number_w + 2) as f32 * char_w + 4.0 * PAD;
        Self {
            mono,
            title: FontId::proportional(13.5),
            char_w,
            text: ui.visuals().text_color(),
            weak: ui.visuals().weak_text_color(),
            strong: ui.visuals().strong_text_color(),
            added_bg,
            removed_bg,
            added_gutter,
            removed_gutter,
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

/// Le righe del diff, virtualizzate: si disegnano solo quelle visibili, anche con migliaia di
/// righe. Il testo scorre in orizzontale; i numeri di riga restano fermi a sinistra.
fn rows(ui: &mut Ui, doc: &DiffDoc, state: &LineState<'_>, actions: &mut Vec<Action>) {
    let style = Style::new(ui, doc);
    let buttons = state.buttons;
    let selectable_lines = buttons.is_some();
    let (pointer, down) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.primary_down()));
    if state.selection.is_dragging() && !down {
        actions.push(Action::DragLines(Drag::End));
    }
    // Larghezza del contenuto: la riga più larga, più il segno di taglio.
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
                // I numeri restano visibili a sinistra anche scorrendo in orizzontale.
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
                        actions.push(Action::SelectLine { row: index, shift });
                    } else if response.drag_started() {
                        actions.push(Action::DragLines(Drag::Start(index)));
                    } else if state.selection.is_dragging()
                        && down
                        && pointer.is_some_and(|p| rect.contains(p))
                    {
                        actions.push(Action::DragLines(Drag::Over(index)));
                    }
                }
                if let (Row::Hunk(_, h), Some(b)) = (row, buttons) {
                    hunk_button(ui, rect, *h, b, actions);
                }
            }
        });
}

/// Come evidenziare una riga cambiata.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    None,
    Hover,
    Selected,
}

/// "Stage hunk"/"Unstage hunk" a destra dell'intestazione, nella parte visibile.
fn hunk_button(ui: &mut Ui, rect: Rect, hunk: usize, b: LineButtons, actions: &mut Vec<Action>) {
    let right = rect.right().min(ui.clip_rect().right()) - PAD;
    let cell = Rect::from_min_max(pos2(right - 140.0, rect.top()), pos2(right, rect.bottom()));
    let mut cell = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(cell)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    let verb = lines::verb(b.direction);
    let help = match b.direction {
        project_domain::PatchDirection::Stage => {
            "Mette in stage tutte le righe cambiate di questo hunk"
        }
        project_domain::PatchDirection::Unstage => {
            "Toglie dallo stage tutte le righe cambiate di questo hunk; il worktree non cambia"
        }
    };
    let button = cell
        .add_enabled(b.hunks, egui::Button::new(format!("{verb} hunk")).small())
        .on_hover_text(help)
        .on_disabled_hover_text("Attendere la fine della scrittura in corso");
    if button.clicked() {
        actions.push(Action::WriteHunk(hunk));
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
            // Il titolo resta visibile scorrendo: parte dal bordo sinistro della vista.
            let left = gutter.left() + PAD;
            let title = painter.text(
                pos2(left, mid),
                Align2::LEFT_CENTER,
                file_title(file),
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
            paint_text(
                ui,
                &line.content.to_string_lossy(),
                pos2(text_x, mid),
                style,
            );

            // Colonna dei numeri sopra il testo, opaca: copre il testo che le scorre sotto.
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
                // Barra piena a sinistra e bordo: la selezione si vede anche senza colori.
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

/// Il contenuto di una riga, con tab espansi, caratteri di controllo tenui e il segno di
/// taglio se è troppo lunga.
fn paint_text(ui: &Ui, content: &str, at: egui::Pos2, style: &Style) {
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
            &format!(" … (altri {} caratteri)", shown.cut),
            0.0,
            format(style.weak),
        );
    }
    let galley = ui.painter().layout_job(job);
    let top = at.y - galley.size().y / 2.0;
    ui.painter().galley(pos2(at.x, top), galley, style.text);
}
