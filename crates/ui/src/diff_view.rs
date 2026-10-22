//! Disegno della vista del diff, al posto del grafo (`impl-docs/ARCHITETTURA-git.md` §8).
//! Legge lo stato di [`crate::diff`] e produce [`Action`]; non cambia niente direttamente.

use crate::Action;
use crate::diff::{
    DiffDoc, DiffSource, DiffTarget, Pane, Row, display_line, file_summary, file_title,
    line_number, line_prefix,
};
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, pos2, vec2};
use project_domain::LineKind;

/// Altezza di una riga del diff: tutte uguali, per `show_rows`.
const ROW_HEIGHT: f32 = 18.0;
/// Margine prima del testo e tra le colonne.
const PAD: f32 = 6.0;

pub(crate) fn diff_panel(ui: &mut Ui, pane: Pane<'_>, actions: &mut Vec<Action>) {
    let target = match pane {
        Pane::Closed => return,
        Pane::Loading(t) | Pane::Loaded(t, _) | Pane::Failed(t, _) => t,
    };
    toolbar(ui, target, pane, actions);
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
        Pane::Loaded(_, doc) => rows(ui, doc),
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
            gutter_w,
            number_w,
        }
    }
}

/// Le righe del diff, virtualizzate: si disegnano solo quelle visibili, anche con migliaia di
/// righe. Il testo scorre in orizzontale; i numeri di riga restano fermi a sinistra.
fn rows(ui: &mut Ui, doc: &DiffDoc) {
    let style = Style::new(ui, doc);
    // Larghezza del contenuto: la riga più larga, più il segno di taglio.
    let text_w = (doc.max_cols + 28) as f32 * style.char_w;
    let content_w = style.gutter_w + PAD + text_w;
    egui::ScrollArea::both()
        .id_salt("diff")
        .auto_shrink(false)
        .show_rows(ui, ROW_HEIGHT, doc.rows.len(), |ui, range| {
            let width = content_w.max(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &doc.rows[range] {
                let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
                // I numeri restano visibili a sinistra anche scorrendo in orizzontale.
                let gutter_left = rect.left().max(ui.clip_rect().left());
                let gutter = Rect::from_min_size(
                    pos2(gutter_left, rect.top()),
                    vec2(style.gutter_w, ROW_HEIGHT),
                );
                paint_row(ui, doc, row, rect, gutter, &style);
            }
        });
}

fn paint_row(ui: &Ui, doc: &DiffDoc, row: &Row, rect: Rect, gutter: Rect, style: &Style) {
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
            painter.text(
                pos2(gutter.left() + PAD, mid),
                Align2::LEFT_CENTER,
                numbers,
                style.mono.clone(),
                style.weak,
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
