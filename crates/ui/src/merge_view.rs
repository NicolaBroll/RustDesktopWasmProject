//! Drawing of the in-progress operation panel and the merge dialogs (S29). The functions
//! read the view and produce [`Action`]; they change nothing directly.

use crate::discard::Phase;
use crate::merge::{self, ContinueDialog, Dialog, Kind, MainlineDialog, OperationInfo};
use crate::view::git_text;
use crate::{Action, RepoView};
use egui::{RichText, Ui};
use project_domain::{Operation, RepoPath};
use project_i18n::tr;

/// The panel below the bar when the repo is in merge, rebase, cherry-pick, revert, bisect or
/// `git am`: what is coming in, the conflicted files (a click opens the conflict
/// editor) and, if the operation can be finished from here, "Continue" (enabled only without conflicts) and
/// "Abort" (asks for confirmation). `operation`: the one from the status, always up to date; the details
/// come from `repo.operation`. `can_write`: no write or dialog in progress.
pub(crate) fn banner(
    ui: &mut Ui,
    repo: &RepoView,
    operation: Operation,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    // Until the reading arrives, or if it already has: the details apply only to the same operation.
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
                    None => tr!("merge-title", name = merge::operation_name(operation)),
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
                        RichText::new(tr!("merge-info-error", error = error))
                            .color(ui.visuals().error_fg_color)
                            .small(),
                    )
                    .wrap(),
                );
            }
        });
}

/// The title tooltip: the incoming commits in full, with the refs pointing at them.
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
        lines.push(tr!("merge-onto", onto = onto.to_string()));
    }
    lines.join("\n")
}

/// "Abort the …" and "Continue the …" (from the right) with the commands that `repo.operation` indicates
/// (merge, cherry-pick, revert), or how to finish from the terminal.
fn buttons(
    ui: &mut Ui,
    info: &OperationInfo,
    operation: Operation,
    can_write: bool,
    actions: &mut Vec<Action>,
) {
    let abort = info.abort_command.as_deref().and_then(Kind::from_abort);
    let resume = info
        .continue_command
        .as_deref()
        .and_then(Kind::from_continue);
    if abort.is_none() && resume.is_none() {
        let why = match operation {
            Operation::CherryPick | Operation::Revert => tr!("merge-sequence-hint"),
            _ => tr!("merge-not-supported-hint"),
        };
        ui.weak(merge::terminal_hint(operation)).on_hover_text(why);
        return;
    }
    if let Some(kind) = abort {
        let name = kind.name();
        let color = if can_write {
            ui.visuals().error_fg_color
        } else {
            ui.visuals().weak_text_color()
        };
        let button = ui
            .add_enabled(
                can_write,
                egui::Button::new(
                    RichText::new(tr!("merge-abort-button", name = name)).color(color),
                ),
            )
            .on_hover_text(tr!(
                "merge-abort-hint",
                name = name,
                backup = kind.abort_backup()
            ))
            .on_disabled_hover_text(tr!("merge-wait"));
        if button.clicked() {
            actions.push(Action::AbortMerge);
        }
    }
    if let Some(kind) = resume {
        let enabled = can_write && info.ready;
        let button = egui::Button::new(
            RichText::new(tr!("merge-continue-button", name = kind.name())).strong(),
        );
        let button = ui
            .add_enabled(enabled, button)
            .on_hover_text(if kind.has_message() {
                tr!("merge-continue-hint", noun = kind.commit_noun())
            } else {
                tr!("merge-continue-hint-rebase")
            })
            .on_disabled_hover_text(if info.ready {
                tr!("merge-wait")
            } else {
                tr!("merge-continue-conflicts")
            });
        if button.clicked() {
            actions.push(Action::ContinueMerge);
        }
    }
}

/// The conflicts row: the first files (a click opens the conflict editor), how many more
/// there are, or that it can be concluded.
fn conflicts_line(ui: &mut Ui, repo: &RepoView, info: &OperationInfo, actions: &mut Vec<Action>) {
    let n = info.conflicts.len();
    if n == 0 {
        let resume = info
            .continue_command
            .as_deref()
            .and_then(Kind::from_continue);
        match resume {
            Some(kind) => ui.label(tr!("merge-no-conflicts-continue", name = kind.name())),
            None => ui.label(tr!("merge-no-conflicts")),
        };
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(tr!("merge-conflicts-label", n = n));
        let open = repo.conflict.editor().map(|e| &e.path);
        for path in info.conflicts.iter().take(merge::BANNER_FILES) {
            file_link(ui, path, open == Some(path), actions);
        }
        if n > merge::BANNER_FILES {
            let more = n - merge::BANNER_FILES;
            if ui
                .link(tr!("merge-more-files", n = more))
                .on_hover_text(tr!("merge-more-files-hint"))
                .clicked()
            {
                actions.push(Action::SelectWorktree);
            }
        }
        ui.weak(tr!("merge-click-to-resolve"));
    });
}

fn file_link(ui: &mut Ui, path: &RepoPath, open: bool, actions: &mut Vec<Action>) {
    let text = RichText::new(path.to_string()).monospace();
    let response = ui
        .selectable_label(open, text)
        .on_hover_text(tr!("merge-file-hint"));
    if response.clicked() {
        actions.push(Action::OpenConflict(path.clone()));
    }
}

/// The open dialog: "Continue", the abort confirmation or the parent of a
/// cherry-pick or revert of a merge commit.
pub(crate) fn merge_dialog(
    ctx: &egui::Context,
    repo: &RepoView,
    dialog: &Dialog,
    actions: &mut Vec<Action>,
) {
    let modal = egui::Modal::new(egui::Id::unique("merge-dialog")).show(ctx, |ui| {
        ui.set_max_width(520.0);
        match dialog {
            Dialog::Continue(dialog) => continue_dialog(ui, repo, dialog, actions),
            Dialog::Abort(kind, Phase::Confirm { reason }) => {
                abort_dialog(ui, *kind, reason, actions)
            }
            Dialog::Abort(..) => {}
            Dialog::Mainline(dialog) => mainline_dialog(ui, dialog, actions),
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
    let kind = dialog.kind;
    if !kind.has_message() {
        return rebase_continue_dialog(ui, repo, dialog, actions);
    }
    let noun = kind.commit_noun();
    ui.heading(tr!("merge-finish-heading", name = kind.name()));
    ui.add_space(6.0);
    let into = match &repo.head {
        project_domain::Head::Branch { name, .. } => tr!("merge-into-branch", branch = name),
        project_domain::Head::Detached(id) => tr!("merge-into-detached", id = id.short()),
    };
    ui.add(egui::Label::new(tr!("merge-finish-text", noun = &noun, into = into)).wrap());
    ui.add_space(8.0);
    ui.label(tr!("merge-message-label", noun = &noun));
    let mut message = dialog.message.clone();
    let edit = ui.add(
        egui::TextEdit::multiline(&mut message)
            .id(ui.make_persistent_id("merge-message"))
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
            .add_enabled(
                enabled,
                egui::Button::new(tr!("merge-create-button", noun = &noun)),
            )
            .on_disabled_hover_text(if dialog.sending {
                tr!("merge-committing-hint")
            } else {
                tr!("merge-write-message")
            });
        if button.clicked() {
            actions.push(Action::SubmitContinueMerge);
        }
        if ui
            .add_enabled(!dialog.sending, egui::Button::new(tr!("merge-close")))
            .clicked()
        {
            actions.push(Action::CloseMergeDialog);
        }
        if dialog.sending {
            ui.spinner();
            ui.label(tr!("merge-committing"));
        }
    });
}

/// "Continue" of a rebase: no message (the commit it stopped on keeps its own), git goes on with
/// the remaining commits and may stop again on the next conflict.
fn rebase_continue_dialog(
    ui: &mut Ui,
    repo: &RepoView,
    dialog: &ContinueDialog,
    actions: &mut Vec<Action>,
) {
    let name = dialog.kind.name();
    ui.heading(tr!("merge-finish-heading", name = name));
    ui.add_space(6.0);
    let branch = repo
        .merge
        .info()
        .and_then(|info| info.rebase_branch.as_deref())
        .map(|b| b.strip_prefix("refs/heads/").unwrap_or(b));
    let text = match branch {
        Some(branch) => tr!("merge-rebase-continue-text-branch", branch = branch),
        None => tr!("merge-rebase-continue-text"),
    };
    ui.add(egui::Label::new(text).wrap());
    if let Some(error) = &dialog.error {
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap());
        git_text(ui, dialog.git.as_deref());
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = ui
            .add_enabled(
                !dialog.sending,
                egui::Button::new(
                    RichText::new(tr!("merge-rebase-continue-button", name = name)).strong(),
                ),
            )
            .on_disabled_hover_text(tr!("merge-continuing"));
        if button.clicked() {
            actions.push(Action::SubmitContinueMerge);
        }
        if ui
            .add_enabled(!dialog.sending, egui::Button::new(tr!("merge-close")))
            .clicked()
        {
            actions.push(Action::CloseMergeDialog);
        }
        if dialog.sending {
            ui.spinner();
            ui.label(tr!("merge-continuing"));
        }
    });
}

fn abort_dialog(ui: &mut Ui, kind: Kind, reason: &str, actions: &mut Vec<Action>) {
    let name = kind.name();
    ui.heading(
        RichText::new(tr!("merge-abort-heading", name = name)).color(ui.visuals().error_fg_color),
    );
    ui.add_space(6.0);
    let text = if kind == Kind::Rebase {
        tr!("merge-abort-text-rebase")
    } else {
        tr!("merge-abort-text", name = name)
    };
    ui.add(egui::Label::new(text).wrap());
    ui.add_space(6.0);
    ui.strong(tr!("merge-what-happens"));
    egui::ScrollArea::vertical()
        .id_salt("merge-abort-reason")
        .max_height(180.0)
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(reason).monospace()).wrap());
        });
    ui.add_space(6.0);
    ui.add(egui::Label::new(tr!("merge-abort-backup", backup = kind.abort_backup())).wrap());
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let button = egui::Button::new(
            RichText::new(tr!("merge-abort-confirm", name = name))
                .color(ui.visuals().strong_text_color()),
        )
        .fill(ui.visuals().error_fg_color.gamma_multiply(0.35));
        if ui.add(button).clicked() {
            actions.push(Action::ConfirmAbortMerge);
        }
        if ui.button(tr!("merge-dont-abort")).clicked() {
            actions.push(Action::CloseMergeDialog);
        }
    });
}

/// The reference parent for the cherry-pick or revert of a merge commit: the
/// changes are those of the merge relative to the chosen parent (`mainline`).
fn mainline_dialog(ui: &mut Ui, dialog: &MainlineDialog, actions: &mut Vec<Action>) {
    let kind = dialog.pick.kind();
    ui.heading(tr!("merge-mainline-heading", title = kind.title()));
    ui.add_space(6.0);
    let (what, n, into) = (&dialog.what, dialog.parents.len(), &dialog.into);
    let text = match dialog.pick {
        merge::Pick::CherryPick => {
            tr!(
                "merge-mainline-text-cherry-pick",
                what = what,
                n = n,
                into = into
            )
        }
        merge::Pick::Revert => tr!(
            "merge-mainline-text-revert",
            what = what,
            n = n,
            into = into
        ),
    };
    ui.add(egui::Label::new(text).wrap());
    ui.add_space(8.0);
    for (i, (id, name, summary)) in dialog.parents.iter().enumerate() {
        let n = i + 1;
        let mut text = format!("{n}. {name}");
        if *name != id.short() {
            text.push_str(&format!(" ({})", id.short()));
        }
        if let Some(summary) = summary {
            text.push_str(&format!(" — {summary}"));
        }
        if ui.radio(dialog.mainline == n, text).clicked() {
            actions.push(Action::Mainline(n));
        }
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if ui
            .button(RichText::new(kind.title()).strong())
            .on_hover_text(tr!("merge-mainline-hint", n = dialog.mainline))
            .clicked()
        {
            actions.push(Action::SubmitMainline);
        }
        if ui.button(tr!("merge-close")).clicked() {
            actions.push(Action::CloseMergeDialog);
        }
    });
}
