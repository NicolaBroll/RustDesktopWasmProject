//! Drawing of the panels. The functions read the view and produce [`Action`]; they do not change
//! anything directly.

use crate::branch::{Dialog, NameKind, Start};
use crate::commit::{CommitDraft, ConfirmAmend, Gate};
use crate::details::Shown;
use crate::diff::{DiffSource, DiffTarget};
use crate::discard::{self, Discard, UndoOp};
use crate::fetch::{self, Failure};
use crate::format::{self, change_letter, date_time, date_time_zone, file_path, file_score};
use crate::graph::{self, LabelKind, RefLabel};
use crate::list::{Anchor, CommitList};
use crate::remotes::{
    self, AddDialog, AddFields, DeleteBranchDialog, EditDialog, EditKind, RemoteText, RemoveDialog,
};
use crate::search::{SearchBox, State as SearchState};
use crate::stage::{self, PendingWrite, Side, StageItem, StageOp};
use crate::stash::{DropDialog, PushDialog, PushFields, StashText};
use crate::sync::Window;
use crate::{Action, Job, ListViewport, RepoView};
use crate::{icons, theme};
use egui::{Align2, Color32, FontId, RichText, Sense, Ui, vec2};
use project_domain::{
    Change, ChangedFile, Commit, Conflict, Head, Oid, RefInfo, RefKind, Signature, Upstream,
};
use project_engine::{Progress, RemoteListEntry, StashEntry};
use project_i18n::tr;

mod auto_fetch_view;
mod branches;
mod clone_view;
mod commits;
mod compare_view;
mod confirm;
mod conflict_view;
mod credentials_view;
mod details_view;
mod file_view;
mod optimize_view;
mod rebase_view;
mod refs;
mod remotes_view;
mod reset_view;
mod settings_view;
mod stage_view;
mod stash_view;
mod submodule_view;
mod sync_view;
mod tab_strip;
mod title_bar;
mod top_bar;

pub(crate) use auto_fetch_view::auto_fetch_notice;
use auto_fetch_view::offline_icon;
pub(crate) use branches::branch_dialog;
pub(crate) use clone_view::clone_dialog;
use clone_view::create_buttons;
pub(crate) use commits::{ListOptions, commit_list};
pub(crate) use compare_view::compare_panel;
pub(crate) use confirm::{confirm_amend, confirm_clean, confirm_discard, confirm_write};
pub(crate) use conflict_view::conflict_editor;
pub(crate) use credentials_view::{credential_dialog, credential_field_id};
pub(crate) use details_view::details_panel;
use details_view::*;
use file_view::file_menu_entries;
pub(crate) use file_view::file_view;
pub(crate) use file_view::{launch_entries, set_can_launch};
pub(crate) use optimize_view::optimize_notice;
pub(crate) use rebase_view::rebase_editor;
pub(crate) use refs::refs_panel;
use refs::*;
pub(crate) use remotes_view::remote_dialog;
use remotes_view::*;
pub(crate) use reset_view::reset_dialog;
use reset_view::reset_item;
pub(crate) use settings_view::settings_window;
pub(crate) use stage_view::stage_panel;
use stash_view::*;
pub(crate) use stash_view::{stash_conflicts_window, stash_dialog, stash_panel};
pub(crate) use submodule_view::outdated_notice;
use submodule_view::submodule_section;
use sync_view::*;
pub(crate) use sync_view::{failure_window, sync_window};
pub(crate) use tab_strip::{TabLabel, tab_strip_panel};
pub use title_bar::WindowFrame;
pub(crate) use title_bar::{TitleBar, end_window_move, title_bar_panel, window_edges};
use top_bar::*;
pub(crate) use top_bar::{
    RecentRepo, current_branch, repo_name, status_bar, top_bar, undo_enabled, welcome,
};

/// The network operation in progress (fetch, pull or push), for the bar.
pub(crate) struct Network {
    /// What is being done, e.g. `Pull of "main"`.
    pub what: String,
    /// True after "Cancel", until the command answers.
    pub cancelling: bool,
    /// The latest progress received.
    pub progress: Option<Progress>,
    /// What happens by cancelling.
    pub stop_hint: String,
    /// The "Cancel" action.
    pub cancel: Action,
}

/// The git text in a window, if any.
pub(crate) fn git_text(ui: &mut Ui, git: Option<&str>) {
    if let Some(git) = git.filter(|g| !g.trim().is_empty()) {
        ui.add_space(6.0);
        ui.label(tr!("common-git-message"));
        egui::ScrollArea::vertical()
            .id_salt("git-text")
            .max_height(140.0)
            .show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(git.trim_end()).monospace()).wrap());
            });
    }
}

/// When an entry that writes is disabled because it is waiting: the reason.
fn wait() -> String {
    tr!("common-wait")
}

/// Short clickable id: selects the commit in the list.
fn oid_link(ui: &mut Ui, id: Oid, actions: &mut Vec<Action>) {
    let response = ui
        .link(RichText::new(id.short()).monospace())
        .on_hover_text(tr!("common-oid-hint", id = id.to_string()));
    if response.clicked() {
        actions.push(Action::SelectCommit(id));
    }
}

/// Path on one line, cut with `…` if it does not fit in `width`. For renames and copies
/// the origin is dimmer.
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
