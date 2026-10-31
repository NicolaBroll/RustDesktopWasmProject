use super::*;
use crate::compare::{self, Comparison, Side};

/// The details panel while two revisions are compared: "Compare A ↔ B", the two sides (and the
/// merge base), "Swap" and "Back", then the files that change; a click on a file opens its diff.
pub(crate) fn compare_panel(
    ui: &mut Ui,
    repo: &RepoView,
    comparison: &Comparison,
    open: Option<&DiffTarget>,
    actions: &mut Vec<Action>,
) {
    let answer = match &comparison.answer {
        Some(Ok(compared)) => Some(compared),
        _ => None,
    };
    let from_id = answer.map(|c| c.from).or(comparison.from.id);
    let to_id = answer.map(|c| c.to).or(comparison.to.id);
    let short = |id: Option<Oid>, side: &Side| id.map_or_else(|| side.label.clone(), |i| i.short());
    ui.add_space(4.0);
    let title = tr!(
        "compare-title",
        from = short(from_id, &comparison.from),
        to = short(to_id, &comparison.to)
    );
    ui.add(egui::Label::new(RichText::new(title).heading()).wrap());
    ui.horizontal(|ui| {
        if ui
            .button(tr!("compare-swap"))
            .on_hover_text(tr!("compare-swap-hint"))
            .clicked()
        {
            actions.push(Action::Compare(compare::Action::Swap));
        }
        if ui
            .button(tr!("compare-back"))
            .on_hover_text(tr!("compare-back-hint"))
            .clicked()
        {
            actions.push(Action::Compare(compare::Action::Close));
        }
    });
    ui.add_space(6.0);
    // Rows, not a grid: in a grid the cells do not take the panel width and the subjects would
    // widen the panel instead of being cut.
    side_row(ui, repo, tr!("compare-from"), from_id, &comparison.from);
    side_row(ui, repo, tr!("compare-to"), to_id, &comparison.to);
    if let Some(base) = answer.and_then(|c| c.base) {
        ui.horizontal(|ui| {
            caption(ui, tr!("compare-base"));
            oid_link(ui, base, actions);
        });
    }
    ui.add_space(4.0);
    let note = if comparison.merge_base {
        tr!(
            "compare-brings",
            to = &comparison.to.label,
            from = &comparison.from.label
        )
    } else {
        tr!("compare-plain")
    };
    ui.add(egui::Label::new(RichText::new(note).weak()).wrap());
    ui.add_space(4.0);
    ui.separator();
    match &comparison.answer {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(tr!("compare-loading"));
            });
        }
        Some(Err(error)) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button(tr!("compare-retry")).clicked() {
                actions.push(Action::Compare(compare::Action::Retry));
            }
        }
        Some(Ok(compared)) => {
            let source = compared.source(comparison.merge_base);
            changed_files(ui, source, compared.to, &compared.files, open, actions);
        }
    }
}

/// One side: the short id (full on hover) and the subject of the commit if it is in the list,
/// after the branch name for a branch.
fn side_row(ui: &mut Ui, repo: &RepoView, caption_text: String, id: Option<Oid>, side: &Side) {
    let subject = id
        .and_then(|id| repo.list.index_of(id))
        .and_then(|i| repo.list.commits().get(i))
        .map(|c| c.summary.as_str());
    ui.horizontal(|ui| {
        caption(ui, caption_text);
        if let Some(id) = id {
            ui.add(egui::Label::new(RichText::new(id.short()).monospace()))
                .on_hover_text(id.to_string());
        }
        // A branch: its name first; one label, cut with "…" at the panel's edge.
        let mut job = egui::text::LayoutJob::default();
        let format = |color| egui::TextFormat {
            font_id: theme::body(),
            color,
            ..Default::default()
        };
        if side.id.is_none() {
            job.append(&side.label, 0.0, format(ui.visuals().strong_text_color()));
        }
        let text = subject.unwrap_or(if side.id.is_some() { &side.label } else { "" });
        job.append(text, 6.0, format(ui.visuals().text_color()));
        ui.add(egui::Label::new(job).truncate());
    });
}

/// The weak caption on the left of a row, with a fixed width so the values line up.
fn caption(ui: &mut Ui, text: String) {
    let width = 84.0;
    let response = ui.add(egui::Label::new(RichText::new(text).weak()).truncate());
    let gap = width - response.rect.width();
    if gap > 0.0 {
        ui.add_space(gap);
    }
}
