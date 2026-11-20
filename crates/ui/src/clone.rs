//! Clone and new repository (S46): "Clone…" and "New repository…" in the File menu and on the
//! empty screen open a dialog (URL for the clone, destination folder with "Browse…", optional
//! branch and, for the clone, depth). "Clone" sends `repo.clone` in the background with progress
//! and "Stop"; "Create" sends `repo.init`. Both open the new repo in the session that ran them:
//! [`App`](crate::App) then shows it in a tab and puts it in the recent list. Data only, no egui.
//! Command contract in `impl-docs/git/clone.md`.

use crate::fetch::{self, Failure};
use crate::running::Running;
use crate::{ErrorKind, Ticket};
use project_engine::{CloneRequest, InitRequest, RepoCloneResult};
use project_i18n::tr;
use serde_json::Value;
use std::path::Path;

/// The actions of this area, applied by [`App`](crate::App).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Opens the "Clone…" or "New repository…" dialog.
    Open(Kind),
    /// The fields of that dialog changed.
    Fields(Fields),
    /// "Browse…" in that dialog: the folder dialog, the choice goes into the destination.
    Browse,
    /// "Clone"/"Create" in that dialog.
    Submit,
    /// "Stop" the clone in progress.
    Cancel,
    /// Closes that dialog (not while its command runs).
    Close,
}

/// What the dialog creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `repo.clone`: a copy of a remote repository.
    Clone,
    /// `repo.init`: a new, empty repository.
    Init,
}

impl Kind {
    /// The id of the Registry command.
    pub fn command(self) -> &'static str {
        match self {
            Kind::Clone => "repo.clone",
            Kind::Init => "repo.init",
        }
    }
}

/// The text fields of the dialog, as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fields {
    /// Address or path of the repository to clone (only for [`Kind::Clone`]).
    pub url: String,
    /// The destination folder: absent or empty.
    pub path: String,
    /// Clone: the branch to check out; init: the initial branch. Empty: the default.
    pub branch: String,
    /// Clone: how many commits to download; empty: the whole history.
    pub depth: String,
    /// Clone: "With submodules" (`--recurse-submodules`).
    pub submodules: bool,
}

/// The clone or new-repository dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneDialog {
    pub kind: Kind,
    pub fields: Fields,
    /// The command sent, until it answers: the fields are read-only.
    pub running: Option<Running>,
    /// Why the last attempt failed, with git's text.
    pub failure: Option<Failure>,
    /// The last clone was cancelled: the folder was cleaned up.
    pub cancelled: bool,
    /// The folder a clone is proposed to go into, with the name from the URL added: the one the
    /// app gave on opening (see [`CloneBox::open`]) or the one chosen with "Browse…". `None`:
    /// nothing is proposed.
    base: Option<String>,
    /// `base` was chosen with "Browse…": it is the destination while the URL gives no name.
    base_chosen: bool,
    /// The user typed in the folder field: it is theirs, the URL no longer changes it (until
    /// they empty it).
    path_typed: bool,
}

impl CloneDialog {
    /// Why the fields cannot be sent yet, if they cannot (shown under them).
    pub fn problem(&self) -> Option<String> {
        let depth = self.fields.depth.trim();
        if self.kind == Kind::Clone
            && !depth.is_empty()
            && depth.parse::<u32>().map_or(true, |n| n == 0)
        {
            return Some(tr!("clone-depth-invalid"));
        }
        None
    }

    /// True if "Clone"/"Create" can be pressed: required fields filled, nothing in progress.
    pub fn can_submit(&self) -> bool {
        self.running.is_none()
            && (self.kind == Kind::Init || !self.fields.url.trim().is_empty())
            && !self.fields.path.trim().is_empty()
            && self.problem().is_none()
    }

    /// The parameters of the command, if they can be sent.
    pub fn params(&self) -> Option<Value> {
        if !self.can_submit() {
            return None;
        }
        let f = &self.fields;
        let path = f.path.trim().to_owned();
        let branch = Some(f.branch.trim())
            .filter(|b| !b.is_empty())
            .map(str::to_owned);
        Some(match self.kind {
            Kind::Clone => crate::params(CloneRequest {
                url: f.url.trim().to_owned(),
                path,
                branch,
                depth: f.depth.trim().parse::<u32>().ok(),
                submodules: f.submodules,
            }),
            Kind::Init => crate::params(InitRequest { path, branch }),
        })
    }
}

/// The folder name `git clone` would choose for `url`: its last component without `.git`
/// (`https://host/a/b.git` → `b`, `user@host:x/y` → `y`). `None` if there is none.
pub fn repo_name(url: &str) -> Option<&str> {
    let url = url.trim().trim_end_matches(['/', '\\']);
    let name = url.rsplit(['/', '\\', ':']).next()?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    (!name.is_empty() && name != "." && name != "..").then_some(name)
}

/// The destination proposed for a clone of `url` inside `base`: `base` plus the name from the
/// URL ([`repo_name`]). `None` if the URL gives no name yet.
pub fn proposed_folder(base: &str, url: &str) -> Option<String> {
    let name = repo_name(url)?;
    Some(Path::new(base).join(name).to_string_lossy().into_owned())
}

/// The status-bar message of a finished clone: what was cloned and where, then git's
/// messages (e.g. that the repository is empty).
fn cloned_notice(url: &str, folder: &str, messages: &[String]) -> String {
    let name = repo_name(url).or_else(|| repo_name(folder)).unwrap_or(url);
    let mut notice = tr!("clone-done", name = name, folder = folder);
    for message in messages {
        notice.push(' ');
        notice.push_str(message);
    }
    notice
}

/// What came back from the dialog's command.
#[derive(Debug)]
pub enum Received {
    /// The new repo is open in the session that ran the command: `value` is the result (as for
    /// `repo.open`), `notice` git's messages (e.g. an empty repository was cloned), for the status
    /// bar. The dialog is closed.
    Opened { value: Value, notice: String },
    /// Cancelled or failed: the dialog stays open with the reason; the session opened nothing.
    Failed,
    /// Not the answer awaited: ignored.
    Unexpected,
}

/// The dialog, one at a time.
#[derive(Debug, Default)]
pub struct CloneBox {
    dialog: Option<CloneDialog>,
}

impl CloneBox {
    pub fn dialog(&self) -> Option<&CloneDialog> {
        self.dialog.as_ref()
    }

    /// The command in progress, if any.
    pub fn running(&self) -> Option<Running> {
        self.dialog.as_ref().and_then(|d| d.running)
    }

    /// Opens the dialog for `kind`; an open one with the same kind stays as it is, one of the other
    /// kind is replaced (not while it is running). `base`: the folder where a clone is proposed
    /// to go (the destination follows the URL until the user types one), if the app knows one.
    pub fn open(&mut self, kind: Kind, base: Option<String>) {
        match &self.dialog {
            Some(d) if d.running.is_some() || d.kind == kind => {}
            _ => {
                self.dialog = Some(CloneDialog {
                    kind,
                    fields: Fields::default(),
                    running: None,
                    failure: None,
                    cancelled: false,
                    base: base.filter(|b| !b.trim().is_empty()),
                    base_chosen: false,
                    path_typed: false,
                });
            }
        }
    }

    /// Closes the dialog, not while its command runs (it would go on without anyone showing its
    /// outcome).
    pub fn close(&mut self) {
        if self.running().is_none() {
            self.dialog = None;
        }
    }

    /// The fields typed, ignored while the command runs. For a clone, while the user has not
    /// typed a folder, the folder follows the URL: [`proposed_folder`] in the dialog's base.
    pub fn set_fields(&mut self, mut fields: Fields) {
        let Some(d) = self.dialog.as_mut().filter(|d| d.running.is_none()) else {
            return;
        };
        if fields.path != d.fields.path {
            d.path_typed = !fields.path.trim().is_empty();
        }
        if d.kind == Kind::Clone
            && !d.path_typed
            && let Some(base) = &d.base
        {
            fields.path = match proposed_folder(base, &fields.url) {
                Some(path) => path,
                None if d.base_chosen => base.clone(),
                None => String::new(),
            };
        }
        d.fields = fields;
    }

    /// The folder chosen with "Browse…": for a clone the folder that will contain it (the
    /// destination is the folder plus the name from the URL, and goes on following the URL), for
    /// a new repository the folder itself.
    pub fn set_folder(&mut self, folder: &str) {
        let Some(d) = self.dialog.as_mut().filter(|d| d.running.is_none()) else {
            return;
        };
        let proposed = match d.kind {
            Kind::Clone => {
                d.base = Some(folder.to_owned());
                d.base_chosen = true;
                d.path_typed = false;
                proposed_folder(folder, &d.fields.url)
            }
            Kind::Init => None,
        };
        d.fields.path = proposed.unwrap_or_else(|| folder.to_owned());
    }

    /// The command and parameters to send for "Clone"/"Create", if they can be sent.
    pub fn submit(&self) -> Option<(&'static str, Value)> {
        let d = self.dialog.as_ref()?;
        Some((d.kind.command(), d.params()?))
    }

    /// The command of [`CloneBox::submit`] started with `ticket`.
    pub fn started(&mut self, ticket: Ticket) {
        if let Some(d) = &mut self.dialog {
            d.failure = None;
            d.cancelled = false;
            d.running = Some(Running::new(ticket));
        }
    }

    /// "Stop" during the clone: the ticket to cancel, only once.
    pub fn cancel(&mut self) -> Option<Ticket> {
        self.dialog.as_mut()?.running.as_mut()?.cancel()
    }

    /// The answer of the command sent with `ticket`.
    pub fn receive(
        &mut self,
        ticket: Ticket,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(d) = self
            .dialog
            .as_mut()
            .filter(|d| d.running.is_some_and(|r| r.ticket == ticket))
        else {
            return Received::Unexpected;
        };
        d.running = None;
        let error = match result {
            Ok(value) => {
                // `repo.init` gives a `RepoOpenResult`, without git's messages.
                let notice = match d.kind {
                    Kind::Clone => {
                        let url = d.fields.url.trim();
                        match serde_json::from_value::<RepoCloneResult>(value.clone()) {
                            Ok(r) => cloned_notice(url, &r.opened.path, &r.messages),
                            Err(_) => cloned_notice(url, d.fields.path.trim(), &[]),
                        }
                    }
                    Kind::Init => String::new(),
                };
                self.dialog = None;
                return Received::Opened { value, notice };
            }
            Err(error) => error,
        };
        let who = tr!("clone-who", url = d.fields.url.trim());
        d.failure = match kind {
            // A credential question cancelled: as "Stop", the folder is put back.
            Some(ErrorKind::Cancelled | ErrorKind::AuthCancelled) => {
                d.cancelled = true;
                None
            }
            Some(ErrorKind::Auth) => Some(fetch::auth_failure(&who, detail)),
            Some(ErrorKind::RemoteUnreachable) => Some(fetch::unreachable_failure(&who, detail)),
            Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams) => Some(Failure {
                title: tr!("clone-not-possible-title"),
                explanation: error,
                git: detail,
                stash: false,
                retry: None,
                identity: false,
            }),
            _ => Some(Failure {
                title: match d.kind {
                    Kind::Clone => tr!("clone-failed-title"),
                    Kind::Init => tr!("clone-init-failed-title"),
                },
                explanation: error,
                git: detail,
                stash: false,
                retry: None,
                identity: false,
            }),
        };
        Received::Failed
    }
}

#[cfg(test)]
#[path = "clone_tests.rs"]
mod tests;
