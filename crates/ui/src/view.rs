//! Disegno dei pannelli. Le funzioni leggono la vista e producono [`Action`]; non cambiano
//! niente direttamente.

use crate::format::{date_time, date_time_zone};
use crate::{Action, Job, RepoView};
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

pub(crate) fn commit_list(ui: &mut Ui, repo: &RepoView, actions: &mut Vec<Action>) {
    if repo.commits.is_empty() {
        ui.centered_and_justified(|ui| ui.weak("Nessun commit."));
        return;
    }
    let font = FontId::proportional(13.5);
    let mono = FontId::monospace(12.5);
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    let rows = repo.commits.len() + usize::from(repo.has_more);

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show_rows(ui, ROW_HEIGHT, rows, |ui, range| {
            for index in range {
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                let Some(commit) = repo.commits.get(index) else {
                    ui.painter().text(
                        rect.left_center() + vec2(8.0, 0.0),
                        Align2::LEFT_CENTER,
                        "… altri commit (arrivano con lo scorrimento a pagine)",
                        font.clone(),
                        weak,
                    );
                    continue;
                };
                let selected = repo.selected == Some(index);
                if selected {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                } else if response.hovered() {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                if response.clicked() {
                    actions.push(Action::Select(index));
                }

                // Colonne: id | oggetto (elastico) | autore | data.
                let date_w = 120.0;
                let author_w = (rect.width() * 0.2).clamp(80.0, 180.0);
                let id_w = 70.0;
                let summary_w = (rect.width() - id_w - author_w - date_w).max(40.0);
                let columns = [
                    (id_w, commit.id.short(), mono.clone(), weak),
                    (summary_w, commit.summary.clone(), font.clone(), text),
                    (author_w, commit.author.name.clone(), font.clone(), weak),
                    (date_w, date_time(&commit.author), font.clone(), weak),
                ];
                let mut x = rect.left() + 8.0;
                for (width, value, font, color) in columns {
                    let cell = egui::Rect::from_min_size(
                        egui::pos2(x, rect.top()),
                        vec2(width - 8.0, rect.height()),
                    );
                    ui.painter_at(cell).text(
                        cell.left_center(),
                        Align2::LEFT_CENTER,
                        value,
                        font,
                        color,
                    );
                    x += width;
                }
            }
        });
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
