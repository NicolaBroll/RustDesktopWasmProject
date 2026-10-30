//! Clone and new repository (S46): "Clone…" and "New repository…" in the File menu and on the
//! empty screen open a dialog (URL for the clone, destination folder with "Browse…", optional
//! branch and, for the clone, depth). "Clone" sends `repo.clone` in the background with progress
//! and "Cancel"; "Create" sends `repo.init`. Both open the new repo in the session that ran them:
//! [`App`](crate::App) then shows it in a tab and puts it in the recent list. Data only, no egui.
//! Command contract in `impl-docs/git/clone.md`.

use crate::fetch::{self, Failure};
use crate::{ErrorKind, Ticket};
use project_engine::{CloneRequest, InitRequest, RepoCloneResult};
use project_i18n::tr;
use serde_json::Value;
use std::path::Path;

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
}

/// The dialog's command in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Running {
    pub ticket: Ticket,
    /// True after "Cancel", until the command answers.
    pub cancelling: bool,
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
    /// kind is replaced (not while it is running).
    pub fn open(&mut self, kind: Kind) {
        match &self.dialog {
            Some(d) if d.running.is_some() || d.kind == kind => {}
            _ => {
                self.dialog = Some(CloneDialog {
                    kind,
                    fields: Fields::default(),
                    running: None,
                    failure: None,
                    cancelled: false,
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

    /// The fields typed, ignored while the command runs.
    pub fn set_fields(&mut self, fields: Fields) {
        if let Some(d) = self.dialog.as_mut().filter(|d| d.running.is_none()) {
            d.fields = fields;
        }
    }

    /// The folder chosen with "Browse…": for a clone the folder that will contain it (the
    /// destination is the folder plus the name from the URL), for a new repository the folder
    /// itself.
    pub fn set_folder(&mut self, folder: &str) {
        let Some(d) = self.dialog.as_mut().filter(|d| d.running.is_none()) else {
            return;
        };
        d.fields.path = match (d.kind, repo_name(&d.fields.url)) {
            (Kind::Clone, Some(name)) => Path::new(folder).join(name).to_string_lossy().into(),
            _ => folder.to_owned(),
        };
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
            d.running = Some(Running {
                ticket,
                cancelling: false,
            });
        }
    }

    /// "Cancel" during the clone: the ticket to cancel, only once.
    pub fn cancel(&mut self) -> Option<Ticket> {
        let running = self.dialog.as_mut()?.running.as_mut()?;
        if running.cancelling {
            return None;
        }
        running.cancelling = true;
        Some(running.ticket)
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
                let messages = match d.kind {
                    Kind::Clone => serde_json::from_value::<RepoCloneResult>(value.clone())
                        .map(|r| r.messages)
                        .unwrap_or_default(),
                    Kind::Init => Vec::new(),
                };
                self.dialog = None;
                return Received::Opened {
                    value,
                    notice: messages.join(" "),
                };
            }
            Err(error) => error,
        };
        let who = tr!("clone-who", url = d.fields.url.trim());
        d.failure = match kind {
            Some(ErrorKind::Cancelled) => {
                d.cancelled = true;
                None
            }
            Some(ErrorKind::Auth) => Some(fetch::auth_failure(&who, detail)),
            Some(ErrorKind::RemoteUnreachable) => Some(fetch::unreachable_failure(&who, detail)),
            Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams) => Some(Failure {
                title: tr!("clone-not-possible-title"),
                explanation: error,
                git: detail,
            }),
            _ => Some(Failure {
                title: match d.kind {
                    Kind::Clone => tr!("clone-failed-title"),
                    Kind::Init => tr!("clone-init-failed-title"),
                },
                explanation: error,
                git: detail,
            }),
        };
        Received::Failed
    }
}

#[cfg(test)]
#[path = "clone_tests.rs"]
mod tests;
