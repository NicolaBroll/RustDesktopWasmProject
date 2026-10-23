//! Disegno dei pannelli. Le funzioni leggono la vista e producono [`Action`]; non cambiano
//! niente direttamente.

use crate::branch::{self, Dialog, NameKind, Start};
use crate::commit::{self, CommitDraft, ConfirmAmend, Gate};
use crate::details::Shown;
use crate::diff::{DiffSource, DiffTarget};
use crate::discard::{self, Discard, UndoOp};
use crate::fetch::{self, Failure};
use crate::format::{self, change_letter, date_time, date_time_zone, file_path, file_score};
use crate::graph::{self, LabelKind, RefLabel};
use crate::list::{Anchor, CommitList};
use crate::merge;
use crate::remotes::{self, AddDialog, AddFields, Remote, RemoveDialog};
use crate::stage::{self, PendingWrite, Side, StageItem, StageOp};
use crate::stash::{self, DropDialog, PushDialog, PushFields, Stash};
use crate::sync::{self, Window};
use crate::{Action, Job, ListViewport, RepoView};
use egui::{Align2, Color32, FontId, RichText, Sense, Ui, vec2};
use project_domain::{
    Change, ChangedFile, Commit, Conflict, Head, Oid, RefInfo, RefKind, Signature, Upstream,
};
use project_engine::Progress;

mod branches;
mod commits;
mod confirm;
mod conflict_view;
mod details_view;
mod refs;
mod remotes_view;
mod stage_view;
mod stash_view;
mod sync_view;
mod top_bar;

pub(crate) use branches::branch_dialog;
pub(crate) use commits::commit_list;
pub(crate) use confirm::{confirm_amend, confirm_discard, confirm_write};
pub(crate) use conflict_view::conflict_editor;
pub(crate) use details_view::details_panel;
use details_view::*;
pub(crate) use refs::refs_panel;
use refs::*;
pub(crate) use remotes_view::remote_dialog;
use remotes_view::*;
pub(crate) use stage_view::stage_panel;
use stash_view::*;
pub(crate) use stash_view::{stash_conflicts_window, stash_dialog, stash_panel};
use sync_view::*;
pub(crate) use sync_view::{failure_window, sync_window};
use top_bar::*;
pub(crate) use top_bar::{status_bar, top_bar, welcome};

/// L'operazione di rete in corso (fetch, pull o push), per la barra.
pub(crate) struct Network {
    /// Cosa si sta facendo, es. `Pull di «main»`.
    pub what: String,
    /// Vero dopo "Interrompi", finché il comando non risponde.
    pub cancelling: bool,
    /// L'ultimo avanzamento ricevuto.
    pub progress: Option<Progress>,
    /// Cosa succede interrompendo.
    pub stop_hint: &'static str,
    /// L'azione di "Interrompi".
    pub cancel: Action,
}

/// Il testo di git in una finestra, se c'è.
pub(crate) fn git_text(ui: &mut Ui, git: Option<&str>) {
    if let Some(git) = git.filter(|g| !g.trim().is_empty()) {
        ui.add_space(6.0);
        ui.label("Messaggio di git:");
        egui::ScrollArea::vertical()
            .id_salt("testo-git")
            .max_height(140.0)
            .show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(git.trim_end()).monospace()).wrap());
            });
    }
}

/// Quando una voce che scrive è spenta perché si aspetta: il motivo.
const WAIT: &str = "Aspettare l'operazione in corso (o chiudere il dialogo aperto)";

/// Id breve cliccabile: seleziona il commit nella lista.
fn oid_link(ui: &mut Ui, id: Oid, actions: &mut Vec<Action>) {
    let response = ui
        .link(RichText::new(id.short()).monospace())
        .on_hover_text(format!("{id}\nClic: seleziona il commit"));
    if response.clicked() {
        actions.push(Action::SelectCommit(id));
    }
}

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
