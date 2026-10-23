//! Disegno del riquadro dell'operazione in corso e dei dialoghi del merge (S29). Le funzioni
//! leggono la vista e producono [`Action`]; non cambiano niente direttamente.

use crate::discard::Phase;
use crate::merge::{self, ContinueDialog, Dialog, OperationInfo};
use crate::view::git_text;
use crate::{Action, RepoView};
use egui::{RichText, Ui};
use project_domain::{Operation, RepoPath};

/// Il riquadro sotto la barra quando il repo è in merge, rebase, cherry-pick, revert, bisect o
/// `git am`: cosa sta entrando, i file in conflitto (un clic apre il loro diff nell'area di
/// stage) e, se l'operazione si conclude da qui, "Continua" (attivo solo senza conflitti) e
/// "Annulla" (chiede conferma). `operation`: quella dello stato, sempre aggiornata; i dettagli
/// arrivano da `repo.operation`. `can_write`: nessuna scrittura né dialogo in corso.
pub(crate) fn banner(
    ui: &mut Ui,
    repo: &RepoView,
    operation: Operation,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    // Finché arriva la lettura, o se l'ha già: i dettagli valgono solo per la stessa operazione.
    let info = repo
        .merge
        .info()
        .filter(|info| info.operation == Some(operation));
    let warn = ui.visuals().warn_fg_color;
    egui::Frame::new()
        .fill(warn.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, warn.gamma_multiply(0.6)))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .outer_margin(egui::Margin::symmetric(0, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let title = match info {
                    Some(info) => info.title(),
                    None => format!("{} in corso", merge::operation_name(operation)),
                };
                ui.add(egui::Label::new(RichText::new(title).strong().color(warn)).truncate())
                    .on_hover_text(info.map(hover).unwrap_or_default());
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| match info {
                        Some(info) => buttons(ui, info, operation, can_write, actions),
                        None => {
                            ui.spinner();
                        }
                    },
                );
            });
            if let Some(info) = info {
                conflicts_line(ui, repo, info, actions);
            }
            if let Some(error) = repo.merge.info_error() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("Dettagli dell'operazione non letti: {error}"))
                            .color(ui.visuals().error_fg_color)
                            .small(),
                    )
                    .wrap(),
                );
            }
        });
}

/// Il suggerimento del titolo: i commit in ingresso per esteso, con i ref che ci puntano.
fn hover(info: &OperationInfo) -> String {
    let mut lines: Vec<String> = info
        .incoming
        .iter()
        .map(|i| {
            let mut line = format!("{}  {}", i.id, i.summary.as_deref().unwrap_or(""));
            if !i.refs.is_empty() {
                line.push_str(&format!("\n  {}", i.refs.join(", ")));
            }
            line
        })
        .collect();
    if let Some(onto) = info.onto {
        lines.push(format!("Su {onto}"));
    }
    lines.join("\n")
}

/// "Annulla il merge…" e "Continua il merge…" (da destra), o come concludere da terminale.
fn buttons(
    ui: &mut Ui,
    info: &OperationInfo,
    operation: Operation,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    if info.continue_command.is_none() && info.abort_command.is_none() {
        ui.weak(format!("Si conclude {}", merge::terminal_hint(operation)))
            .on_hover_text(
                "La UI non sa ancora concludere o annullare questa operazione: lo farà con i \
                 passi successivi (cherry-pick e revert, rebase)",
            );
        return;
    }
    if info.abort_command.is_some() {
        let color = if can_write {
            ui.visuals().error_fg_color
        } else {
            ui.visuals().weak_text_color()
        };
        let button = ui
            .add_enabled(
                can_write,
                egui::Button::new(RichText::new("Annulla il merge…").color(color)),
            )
            .on_hover_text(
                "Torna a com'era prima del merge: chiede conferma e spiega cosa si perde. Prima \
                 salva indice e worktree in un backup (refs/project/backup/merge-abort-…)",
            )
            .on_disabled_hover_text("Aspettare l'operazione in corso (o chiudere il dialogo)");
        if button.clicked() {
            actions.push(Action::AbortMerge);
        }
    }
    if info.continue_command.is_some() {
        let enabled = can_write && info.ready;
        let button = egui::Button::new(RichText::new("Continua il merge…").strong());
        let button = ui
            .add_enabled(enabled, button)
            .on_hover_text("Crea il commit di merge: si può cambiare il messaggio")
            .on_disabled_hover_text(if info.ready {
                "Aspettare l'operazione in corso (o chiudere il dialogo)"
            } else {
                "Restano file in conflitto: risolverli e segnarli come risolti (stage) nell'area \
                 delle modifiche"
            });
        if button.clicked() {
            actions.push(Action::ContinueMerge);
        }
    }
}

/// La riga dei conflitti: i primi file (un clic apre il diff nell'area di stage), quanti altri
/// ce ne sono, oppure che si può concludere.
fn conflicts_line(ui: &mut Ui, repo: &RepoView, info: &OperationInfo, actions: &mut Vec<Action>) {
    let n = info.conflicts.len();
    if n == 0 {
        let text = if info.continue_command.is_some() {
            "Nessun file in conflitto: si può concludere con «Continua il merge…»"
        } else {
            "Nessun file in conflitto"
        };
        ui.label(text);
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(match n {
            1 => "1 file in conflitto:".to_owned(),
            n => format!("{n} file in conflitto:"),
        });
        let open = repo
            .diff
            .target()
            .filter(|_| repo.worktree_selected)
            .map(|t| &t.path);
        for path in info.conflicts.iter().take(merge::BANNER_FILES) {
            file_link(ui, path, open == Some(path), actions);
        }
        if n > merge::BANNER_FILES {
            let more = n - merge::BANNER_FILES;
            if ui
                .link(format!("e altri {more}…"))
                .on_hover_text("Apre l'area di stage, dove ci sono tutti")
                .clicked()
            {
                actions.push(Action::SelectWorktree);
            }
        }
        ui.weak("— risolverli e segnarli come risolti nell'area delle modifiche");
    });
}

fn file_link(ui: &mut Ui, path: &RepoPath, open: bool, actions: &mut Vec<Action>) {
    let text = RichText::new(path.to_string()).monospace();
    let response = ui
        .selectable_label(open, text)
        .on_hover_text("Mostra il diff del file nell'area di stage");
    if response.clicked() {
        actions.push(Action::OpenConflict(path.clone()));
    }
}

/// Il dialogo del merge aperto: "Continua il merge" o la conferma dell'annullamento.
pub(crate) fn merge_dialog(
    ctx: &egui::Context,
    repo: &RepoView,
    dialog: &Dialog,
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("dialogo-merge")).show(ctx, |ui| {
        ui.set_max_width(520.0);
        match dialog {
            Dialog::Continue(dialog) => continue_dialog(ui, repo, dialog, actions),
            Dialog::Abort(Phase::Confirm { reason }) => abort_dialog(ui, reason, actions),
            Dialog::Abort(_) => {}
        }
    });
    if modal.should_close() {
        actions.push(Action::CloseMergeDialog);
    }
}

fn continue_dialog(
    ui: &mut Ui,
    repo: &RepoView,
    dialog: &ContinueDialog,
    actions: &mut Vec<Action>,
) {
    ui.heading("Concludi il merge");
    ui.add_space(6.0);
    let into = match &repo.head {
        project_domain::Head::Branch { name, .. } => format!("su «{name}»"),
        project_domain::Head::Detached(id) => format!("su HEAD staccata ({})", id.short()),
    };
    ui.add(
        egui::Label::new(format!(
            "Crea il commit di merge {into} con quello che è in stage. «Annulla» nella barra lo \
             annulla, finché l'app resta aperta."
        ))
        .wrap(),
    );
    ui.add_space(8.0);
    ui.label("Messaggio del commit di merge");
    let mut message = dialog.message.clone();
    let edit = ui.add(
        egui::TextEdit::multiline(&mut message)
            .id(ui.make_persistent_id("messaggio-merge"))
            .desired_rows(5)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace)
            .interactive(!dialog.sending),
    );
    if !dialog.sending && ui.memory(|m| m.focused().is_none()) {
        edit.request_focus();
    }
    if edit.changed() {
        actions.push(Action::MergeMessage(message));
    }
    if let Some(error) = &dialog.error {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap());
        git_text(ui, dialog.git.as_deref());
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let enabled = !dialog.sending && !dialog.is_blank();
        let button = ui
            .add_enabled(enabled, egui::Button::new("Crea il commit di merge"))
            .on_disabled_hover_text(if dialog.sending {
                "Commit in corso"
            } else {
                "Scrivere il messaggio"
            });
        if button.clicked() {
            actions.push(Action::SubmitContinueMerge);
        }
        if ui
            .add_enabled(!dialog.sending, egui::Button::new("Chiudi"))
            .clicked()
        {
            actions.push(Action::CloseMergeDialog);
        }
        if dialog.sending {
            ui.spinner();
            ui.label("Commit…");
        }
    });
}

fn abort_dialog(ui: &mut Ui, reason: &str, actions: &mut Vec<Action>) {
    ui.heading(RichText::new("Annullare il merge?").color(ui.visuals().error_fg_color));
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Il merge in corso si annulla: indice e file tornano come prima del merge, il branch \
             non si muove.",
        )
        .wrap(),
    );
    ui.add_space(6.0);
    ui.strong("Cosa succede:");
    egui::ScrollArea::vertical()
        .id_salt("motivo-annulla-merge")
        .max_height(180.0)
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
        });
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "«Annulla» nella barra in alto non lo ripristina: per riavere le risoluzioni fatte \
             resta il backup (git stash apply <backup>).",
        )
        .wrap(),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = egui::Button::new(
            RichText::new("Annulla il merge").color(ui.visuals().strong_text_color()),
        )
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
        if ui.add(button).clicked() {
            actions.push(Action::ConfirmAbortMerge);
        }
        if ui.button("Non annullare").clicked() {
            actions.push(Action::CloseMergeDialog);
        }
    });
}
