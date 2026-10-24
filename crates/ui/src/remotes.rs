//! Remote management from the "Remotes" section of the refs column (S27): `remote.list`,
//! `remote.add`, `remote.remove`. Data only, no egui: tested with unit tests.
//!
//! The list comes from `remote.list` (also remotes without fetched branches, and names with `/`
//! kept whole), read on open and reread after add, remove, fetch, pull and push. URLs
//! arrive already masked by the engine: passwords or tokens are never seen here.
//!
//! Adding is typed in a dialog (name, URL, "Fetch right away"); engine errors
//! stay in the dialog. With the fetch the dialog shows progress and "Cancel": a failed
//! or cancelled fetch does not undo the add. Removal happens in two steps like
//! deleting a branch: first without `confirm` (the engine always answers with
//! `ConfirmRequired` and the reason), then, only after the click in the red dialog, `"confirm": true`.

use crate::discard::{History, Phase};
use crate::fetch::{self, Failure, FetchResult, RefUpdate};
use crate::{Action, App, Done, Request, remotes};
use crate::{ErrorKind, Ticket};
use project_domain::{RefInfo, Upstream};
use project_engine::mask_url;
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

/// The id of the command that lists the remotes.
pub const LIST: &str = "remote.list";

/// A command that changes the remotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Remove,
}

impl Op {
    /// The id of the Registry command.
    pub fn command(self) -> &'static str {
        match self {
            Op::Add => "remote.add",
            Op::Remove => "remote.remove",
        }
    }
}

/// A configured remote, as `remote.list` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Remote {
    pub name: String,
    /// The fetch URL, with credentials masked; `None` for a remote without URL.
    pub url: Option<String>,
    /// Where the push goes if it is not `url` (masked).
    #[serde(default)]
    pub push_urls: Vec<String>,
    #[serde(default)]
    pub mirror: bool,
    /// How many remote branches `refs/remotes/<name>/...` there are.
    pub branches: usize,
    /// It is the remote of "Fetch" without a choice.
    #[serde(default)]
    pub default: bool,
    /// It is `remote.pushDefault`.
    #[serde(default)]
    pub push_default: bool,
    /// The configured URL contains credentials (saved in clear text by git).
    #[serde(default)]
    pub credentials: bool,
}

impl Remote {
    /// The URL to show.
    pub fn url_text(&self) -> String {
        match &self.url {
            Some(url) => url.clone(),
            None => tr!("remotes-no-url"),
        }
    }

    /// `no branches`, `1 branch`, `3 branches`.
    pub fn branches_text(&self) -> String {
        match self.branches {
            0 => tr!("remotes-no-branches"),
            n => tr!("remotes-branches", n = n),
        }
    }

    /// The labels next to the name, with their explanation.
    pub fn badges(&self) -> Vec<(String, String)> {
        let mut badges = Vec::new();
        if self.default {
            badges.push((
                tr!("remotes-badge-default"),
                tr!("remotes-badge-default-hint"),
            ));
        }
        if self.push_default {
            badges.push((
                tr!("remotes-badge-push"),
                tr!("remotes-badge-push-hint", name = &self.name),
            ));
        }
        if self.mirror {
            badges.push((
                tr!("remotes-badge-mirror"),
                tr!("remotes-badge-mirror-hint"),
            ));
        }
        badges
    }

    /// The text of the row tooltip.
    pub fn tooltip(&self) -> String {
        let mut text = tr!("remotes-tooltip", name = &self.name, url = self.url_text());
        for url in &self.push_urls {
            text.push('\n');
            text.push_str(&tr!("remotes-tooltip-push-url", url = url));
        }
        text.push('\n');
        text.push_str(&self.branches_text());
        if self.credentials {
            text.push('\n');
            text.push_str(&tr!("remotes-tooltip-credentials"));
        }
        text
    }
}

/// Result of `remote.list`.
#[derive(Debug, Clone, Deserialize)]
struct ListResult {
    remotes: Vec<Remote>,
}

/// True if the typed URL contains credentials, with the same rule as the engine
/// ([`mask_url`]): only for the warning in the dialog, before sending it. The engine says so anyway
/// in the `warnings` after the add.
pub fn looks_like_credentials(url: &str) -> bool {
    mask_url(url.trim()).1
}

/// The fields of the "Add remote" dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddFields {
    pub name: String,
    pub url: String,
    /// "Fetch right away".
    pub fetch: bool,
}

/// The "Add remote" dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddDialog {
    pub fields: AddFields,
    /// The engine error from the last submit, shown in the dialog.
    pub error: Option<String>,
    /// True while the command is running.
    pub sending: bool,
    /// The ticket of the running command (for progress and "Cancel").
    pub ticket: Option<Ticket>,
    /// True after "Cancel", until the command answers.
    pub cancelling: bool,
    /// True if the running submit also asked for the fetch.
    pub fetching: bool,
    /// "Hide": the URL with credentials is shown masked even while typing.
    pub hide_url: bool,
}

impl AddDialog {
    /// True if the button is enabled: name and URL typed, no submit in progress. The rest is
    /// checked by the engine.
    pub fn can_submit(&self) -> bool {
        !self.sending && !self.fields.name.trim().is_empty() && !self.fields.url.trim().is_empty()
    }

    /// True if the URL is shown masked in a read-only field: during the submit (the
    /// fetch can take long, the token does not stay in clear on screen) or after "Hide".
    pub fn url_masked(&self) -> bool {
        (self.sending || self.hide_url) && looks_like_credentials(&self.fields.url)
    }

    /// The URL as shown in the field: masked as the engine shows it ([`mask_url`]) if
    /// [`AddDialog::url_masked`], otherwise as typed.
    pub fn shown_url(&self) -> String {
        if self.url_masked() {
            mask_url(self.fields.url.trim()).0
        } else {
            self.fields.url.clone()
        }
    }

    /// The warning to show below the URL, if it contains credentials.
    pub fn credentials_warning(&self) -> Option<String> {
        looks_like_credentials(&self.fields.url).then(|| tr!("remotes-credentials-warning"))
    }

    /// The remote being added, for the status bar.
    pub fn label(&self) -> String {
        let name = self.fields.name.trim();
        if self.fetching {
            tr!("remotes-add-label-fetch", name = name)
        } else {
            tr!("remotes-add-label", name = name)
        }
    }
}

/// The removal of a remote, from the check to the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveDialog {
    pub name: String,
    pub phase: Phase,
}

/// The open remotes dialog (or the removal being checked, still without a dialog).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Add(AddDialog),
    Remove(RemoveDialog),
}

/// The list of remotes and the dialogs to change it: one dialog at a time.
#[derive(Debug, Default)]
pub struct RemotesBox {
    /// The last list read; `None` until it arrives.
    list: Option<Vec<Remote>>,
    /// The error of the last list read.
    list_error: Option<String>,
    dialog: Option<Dialog>,
    /// The fetch after the add failed for a reason to explain.
    failure: Option<Failure>,
}

/// What came back from `remote.add`/`remote.remove`.
#[derive(Debug)]
pub enum Received {
    /// Done: what to update in the view.
    Done(Box<Outcome>),
    /// Refused: the error is in the add dialog, which stays open.
    Inline,
    /// The removal needs confirmation: the dialog opens.
    Confirm,
    /// Refused or failed: the message to show; with `reload` everything is reloaded.
    Failed { message: String, reload: bool },
}

/// The result of a successful command, for the view.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub notice: String,
    /// The new refs, if the command reloaded them.
    pub refs: Option<Vec<RefInfo>>,
    /// The new list of remotes, if the command reloaded it.
    pub remotes: Option<Vec<Remote>>,
    /// A problem after the successful operation (refs or remotes not reread, fetch failed).
    pub warning: Option<String>,
    pub history: History,
}

impl RemotesBox {
    /// The list of remotes, if it arrived.
    pub fn list(&self) -> Option<&[Remote]> {
        self.list.as_deref()
    }

    /// The error of the last list read.
    pub fn list_error(&self) -> Option<&str> {
        self.list_error.as_deref()
    }

    /// The names of the remotes, in order (empty until the list arrives).
    pub fn names(&self) -> Vec<String> {
        self.list.iter().flatten().map(|r| r.name.clone()).collect()
    }

    /// The answer of `remote.list`.
    pub fn receive_list(&mut self, result: Result<Value, String>) {
        match result.and_then(|value| {
            serde_json::from_value::<ListResult>(value).map_err(|e| {
                tr!(
                    "remotes-unexpected-result",
                    command = LIST,
                    error = e.to_string()
                )
            })
        }) {
            Ok(list) => {
                self.list = Some(list.remotes);
                self.list_error = None;
            }
            Err(e) => self.list_error = Some(e),
        }
    }

    fn set_list(&mut self, remotes: Vec<Remote>) {
        self.list = Some(remotes);
        self.list_error = None;
    }

    /// The dialog to draw: the removal is shown only when it asks for confirmation.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref().filter(|d| match d {
            Dialog::Remove(remove) => matches!(remove.phase, Phase::Confirm { .. }),
            Dialog::Add(_) => true,
        })
    }

    /// True if a dialog is open: no other writes, keys go to it.
    pub fn modal_open(&self) -> bool {
        self.dialog().is_some()
    }

    /// The fetch after the add failed, to explain in a window.
    pub fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    /// Closes the fetch error window.
    pub fn dismiss(&mut self) {
        self.failure = None;
    }

    /// Opens the "Add remote" dialog: proposed name `origin` if there is no remote
    /// with that name yet, fetch enabled.
    pub fn open_add(&mut self) -> bool {
        if self.dialog.is_some() {
            return false;
        }
        let taken = self.list.iter().flatten().any(|r| r.name == "origin");
        self.dialog = Some(Dialog::Add(AddDialog {
            fields: AddFields {
                name: if taken {
                    String::new()
                } else {
                    "origin".into()
                },
                url: String::new(),
                fetch: true,
            },
            error: None,
            sending: false,
            ticket: None,
            cancelling: false,
            fetching: false,
            hide_url: false,
        }));
        true
    }

    /// The fields of the add dialog changed.
    pub fn set_fields(&mut self, fields: AddFields) {
        if let Some(Dialog::Add(dialog)) = &mut self.dialog
            && !dialog.sending
        {
            dialog.fields = fields;
        }
    }

    /// "Show"/"Hide" the URL with credentials in the dialog.
    pub fn hide_url(&mut self, hide: bool) {
        if let Some(Dialog::Add(dialog)) = &mut self.dialog {
            dialog.hide_url = hide;
        }
    }

    /// Submit of the add dialog: parameters and description. `None` if it cannot be done.
    pub fn submit_add(&mut self) -> Option<(Value, String)> {
        let Some(Dialog::Add(dialog)) = &mut self.dialog else {
            return None;
        };
        if !dialog.can_submit() {
            return None;
        }
        dialog.sending = true;
        dialog.error = None;
        dialog.cancelling = false;
        dialog.fetching = dialog.fields.fetch;
        self.failure = None;
        let params = json!({
            "name": dialog.fields.name.trim(),
            "url": dialog.fields.url.trim(),
            "fetch": dialog.fields.fetch,
        });
        Some((params, dialog.label()))
    }

    /// The add sent with [`RemotesBox::submit_add`] started with `ticket`.
    pub fn add_started(&mut self, ticket: Ticket) {
        if let Some(Dialog::Add(dialog)) = &mut self.dialog {
            dialog.ticket = Some(ticket);
        }
    }

    /// The add in progress with the fetch: the ticket for progress.
    pub fn running_fetch(&self) -> Option<&AddDialog> {
        match &self.dialog {
            Some(Dialog::Add(dialog)) if dialog.sending && dialog.fetching => Some(dialog),
            _ => None,
        }
    }

    /// "Cancel" the fetch of the add: the ticket to cancel, only once. The remote
    /// stays added.
    pub fn cancel(&mut self) -> Option<Ticket> {
        let Some(Dialog::Add(dialog)) = &mut self.dialog else {
            return None;
        };
        if !dialog.sending || !dialog.fetching || dialog.cancelling {
            return None;
        }
        dialog.cancelling = true;
        dialog.ticket
    }

    /// Starts the removal of the remote `name`: the parameters of the first submit, without `confirm`
    /// (the engine answers with the reason for the confirmation).
    pub fn start_remove(&mut self, name: String) -> Option<Value> {
        if self.dialog.is_some() {
            return None;
        }
        let params = json!({ "name": name });
        self.dialog = Some(Dialog::Remove(RemoveDialog {
            name,
            phase: Phase::Checking,
        }));
        Some(params)
    }

    /// The user confirmed the removal: the parameters with `"confirm": true`.
    pub fn confirm_remove(&mut self) -> Option<(Value, String)> {
        let Some(Dialog::Remove(remove)) = &mut self.dialog else {
            return None;
        };
        if !matches!(remove.phase, Phase::Confirm { .. }) {
            return None;
        }
        remove.phase = Phase::Sending;
        Some((
            json!({ "name": remove.name, "confirm": true }),
            tr!("remotes-remove-label", name = &remove.name),
        ))
    }

    /// Closes the open dialog without sending anything; not during a submit.
    pub fn close(&mut self) {
        let closable = match &self.dialog {
            Some(Dialog::Add(dialog)) => !dialog.sending,
            Some(Dialog::Remove(remove)) => matches!(remove.phase, Phase::Confirm { .. }),
            None => false,
        };
        if closable {
            self.dialog = None;
        }
    }

    /// The answer of the command `op`.
    pub fn receive(
        &mut self,
        op: Op,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let received = match op {
            Op::Add => self.receive_add(result, kind),
            Op::Remove => self.receive_remove(result, kind, detail),
        };
        if let Received::Done(outcome) = &received
            && let Some(remotes) = &outcome.remotes
        {
            self.set_list(remotes.clone());
        }
        received
    }

    fn receive_add(&mut self, result: Result<Value, String>, kind: Option<ErrorKind>) -> Received {
        let Some(Dialog::Add(dialog)) = &mut self.dialog else {
            return unexpected(Op::Add);
        };
        if !dialog.sending {
            return unexpected(Op::Add);
        }
        match result {
            // Name or URL refused, name already taken: nothing changed, fixed in the dialog.
            Err(e) if matches!(kind, Some(ErrorKind::InvalidParams | ErrorKind::NotAllowed)) => {
                dialog.sending = false;
                dialog.ticket = None;
                dialog.fetching = false;
                dialog.error = Some(e);
                Received::Inline
            }
            Err(e) => {
                // A git error during the add: maybe the remote exists, everything is reloaded.
                let name = dialog.fields.name.trim().to_owned();
                self.dialog = None;
                Received::Failed {
                    message: tr!("remotes-add-failed", name = name, error = e),
                    reload: true,
                }
            }
            Ok(value) => {
                self.dialog = None;
                match parse::<AddResult>(Op::Add, value) {
                    Ok(result) => {
                        let (outcome, failure) = result.outcome();
                        self.failure = failure;
                        Received::Done(Box::new(outcome))
                    }
                    Err(message) => Received::Failed {
                        message,
                        reload: true,
                    },
                }
            }
        }
    }

    fn receive_remove(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(Dialog::Remove(remove)) = self.dialog.take() else {
            return unexpected(Op::Remove);
        };
        match result {
            Err(_)
                if kind == Some(ErrorKind::ConfirmRequired) && remove.phase == Phase::Checking =>
            {
                self.dialog = Some(Dialog::Remove(RemoveDialog {
                    name: remove.name,
                    phase: Phase::Confirm {
                        reason: detail.unwrap_or_default(),
                    },
                }));
                Received::Confirm
            }
            Err(e) => {
                let refused = kind == Some(ErrorKind::NotAllowed);
                let message = if refused {
                    tr!("remotes-remove-refused", name = &remove.name, error = e)
                } else {
                    tr!("remotes-remove-failed", name = &remove.name, error = e)
                };
                Received::Failed {
                    message,
                    reload: !refused,
                }
            }
            Ok(value) => match parse::<RemoveResult>(Op::Remove, value) {
                Ok(result) => Received::Done(Box::new(result.outcome())),
                Err(message) => Received::Failed {
                    message,
                    reload: true,
                },
            },
        }
    }
}

fn unexpected(op: Op) -> Received {
    Received::Failed {
        message: tr!("remotes-unexpected-response", command = op.command()),
        reload: true,
    }
}

fn parse<T: for<'de> Deserialize<'de>>(op: Op, value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| {
        tr!(
            "remotes-unexpected-result",
            command = op.command(),
            error = e.to_string()
        )
    })
}

/// `; warning: ...` for each warning.
fn warnings_text(warnings: &[String]) -> String {
    warnings
        .iter()
        .map(|w| tr!("remotes-warning", warning = w))
        .collect()
}

/// The problem after a successful operation: refs or remotes not reread.
fn reread_warning(what: &str, refs: Option<String>, remotes: Option<String>) -> Option<String> {
    let parts: Vec<String> = [
        refs.map(|e| tr!("remotes-refs-not-reread", error = e)),
        remotes.map(|e| tr!("remotes-remotes-not-reread", error = e)),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| {
        tr!(
            "remotes-reread-warning",
            what = what,
            problems = parts.join("; ")
        )
    })
}

/// The fetch done after the add.
#[derive(Debug, Clone, Deserialize)]
struct FetchPart {
    updated: Option<Vec<RefUpdate>>,
    #[serde(default)]
    messages: Vec<String>,
    #[serde(default, deserialize_with = "crate::engine_text::texts")]
    warnings: Vec<String>,
}

/// The failed fetch after the add.
#[derive(Debug, Clone, Deserialize)]
struct FetchError {
    kind: String,
    message: String,
}

/// Result of `remote.add`.
#[derive(Debug, Clone, Deserialize)]
struct AddResult {
    remote: String,
    url: String,
    fetch: Option<FetchPart>,
    fetch_error: Option<FetchError>,
    #[serde(default, deserialize_with = "crate::engine_text::texts")]
    warnings: Vec<String>,
    remotes: Option<Vec<Remote>>,
    remotes_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

/// What to say in the window when the fetch after the add fails: the remote is there.
fn added_anyway() -> String {
    tr!("remotes-added-anyway")
}

impl AddResult {
    fn outcome(self) -> (Outcome, Option<Failure>) {
        let name = &self.remote;
        let mut notice = tr!("remotes-added", name = name, url = &self.url);
        let mut failure = None;
        if let Some(fetch) = self.fetch {
            let summary = FetchResult {
                remote: name.clone(),
                branch: None,
                updated: fetch.updated,
                messages: fetch.messages,
                warnings: fetch.warnings,
                refs: None,
                refs_error: None,
            }
            .outcome()
            .notice;
            notice.push_str(&format!("; {}", crate::lowercase_first(&summary)));
        }
        if let Some(error) = self.fetch_error {
            let who = fetch::who(Some(name));
            let detail = Some(error.message.clone());
            let window = |mut f: Failure| {
                f.explanation = f
                    .explanation
                    .replace(&fetch::nothing_changed(), &added_anyway());
                f
            };
            match error.kind.as_str() {
                "cancelled" => notice.push_str(&tr!("remotes-added-fetch-cancelled")),
                "auth" => {
                    failure = Some(window(fetch::auth_failure(&who, detail)));
                    notice.push_str(&tr!("remotes-added-fetch-auth"));
                }
                "unreachable" => {
                    failure = Some(window(fetch::unreachable_failure(&who, detail)));
                    notice.push_str(&tr!("remotes-added-fetch-unreachable"));
                }
                _ => {
                    failure = Some(Failure {
                        title: tr!("remotes-fetch-failed-title"),
                        explanation: format!("{} {}", error.message, added_anyway()),
                        git: None,
                    });
                    notice.push_str(&tr!("remotes-added-fetch-failed"));
                }
            }
        }
        notice.push_str(&tr!("remotes-added-undo"));
        notice.push_str(&warnings_text(&self.warnings));
        let outcome = Outcome {
            notice,
            refs: self.refs,
            remotes: self.remotes,
            warning: reread_warning(
                &tr!("remotes-added-short"),
                self.refs_error,
                self.remotes_error,
            ),
            history: self.history,
        };
        (outcome, failure)
    }
}

/// The removed remote, as the engine photographs it (the number of remote branches is enough).
#[derive(Debug, Clone, Deserialize)]
struct Removed {
    #[serde(default)]
    refs: Vec<Value>,
}

/// Result of `remote.remove`.
#[derive(Debug, Clone, Deserialize)]
struct RemoveResult {
    remote: String,
    removed: Removed,
    #[serde(default)]
    upstreams_removed: Vec<String>,
    #[serde(default)]
    push_remotes_removed: Vec<String>,
    #[serde(default)]
    push_default_removed: bool,
    #[serde(default)]
    backups: Vec<String>,
    #[serde(default, deserialize_with = "crate::engine_text::texts")]
    warnings: Vec<String>,
    remotes: Option<Vec<Remote>>,
    remotes_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

/// `“a”, “b”`.
fn quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|n| tr!("remotes-quoted", name = n))
        .collect::<Vec<_>>()
        .join(", ")
}

impl RemoveResult {
    fn outcome(self) -> Outcome {
        let mut notice = tr!(
            "remotes-removed",
            name = &self.remote,
            n = self.removed.refs.len()
        );
        if !self.upstreams_removed.is_empty() {
            notice.push_str(&tr!(
                "remotes-removed-upstreams",
                names = quoted(&self.upstreams_removed)
            ));
        }
        if !self.push_remotes_removed.is_empty() {
            notice.push_str(&tr!(
                "remotes-removed-push-remotes",
                names = quoted(&self.push_remotes_removed)
            ));
        }
        if self.push_default_removed {
            notice.push_str(&tr!("remotes-removed-push-default"));
        }
        match self.backups.len() {
            0 => {}
            1 => notice.push_str(&tr!("remotes-removed-backup", backup = &self.backups[0])),
            n => notice.push_str(&tr!("remotes-removed-backups", n = n)),
        }
        notice.push_str(&tr!("remotes-removed-undo"));
        notice.push_str(&warnings_text(&self.warnings));
        Outcome {
            notice,
            refs: self.refs,
            remotes: self.remotes,
            warning: reread_warning(
                &tr!("remotes-removed-short"),
                self.refs_error,
                self.remotes_error,
            ),
            history: self.history,
        }
    }
}

/// The short name of the upstream: `origin/main`.
pub fn upstream_name(upstream: &Upstream) -> &str {
    upstream
        .name
        .strip_prefix("refs/remotes/")
        .unwrap_or(&upstream.name)
}

/// Ahead and behind relative to the upstream, next to a branch: the short text (`↑2 ↓1`,
/// empty if in sync, `upstream gone`) and the explanation for the tooltip.
pub fn ahead_behind(upstream: &Upstream) -> (String, String) {
    let name = upstream_name(upstream);
    match (upstream.ahead, upstream.behind) {
        (Some(ahead), Some(behind)) => {
            let mut short = Vec::new();
            let mut long = Vec::new();
            if ahead > 0 {
                short.push(format!("↑{ahead}"));
                long.push(tr!("remotes-ahead", n = ahead));
            }
            if behind > 0 {
                short.push(format!("↓{behind}"));
                long.push(tr!("remotes-behind", n = behind));
            }
            let long = if long.is_empty() {
                tr!("remotes-in-sync", name = name)
            } else {
                tr!("remotes-relative", counts = long.join(", "), name = name)
            };
            (short.join(" "), long)
        }
        _ => (
            tr!("remotes-upstream-gone"),
            tr!("remotes-upstream-gone-hint", name = name),
        ),
    }
}

impl App {
    /// The actions of this function, dispatched by [`App::apply`].
    pub(crate) fn apply_remotes(&mut self, action: Action) {
        match action {
            Action::AddRemote => {
                if let Some(repo) = &mut self.repo
                    && self.executor.writing().is_none()
                    && !repo.modal_open()
                {
                    repo.remotes.open_add();
                }
            }
            Action::AddRemoteFields(fields) => {
                if let Some(repo) = &mut self.repo {
                    repo.remotes.set_fields(fields);
                }
            }
            Action::HideRemoteUrl(hide) => {
                if let Some(repo) = &mut self.repo {
                    repo.remotes.hide_url(hide);
                }
            }
            Action::SubmitAddRemote => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let Some((params, label)) = repo.remotes.submit_add() else {
                    return;
                };
                repo.write_error = None;
                repo.write_label = Some(label);
                let op = remotes::Op::Add;
                let ticket = self.executor.submit(op.command(), params);
                self.pending.insert(ticket, Request::Remote(generation, op));
                if let Some(repo) = &mut self.repo {
                    repo.remotes.add_started(ticket);
                }
            }
            Action::CancelAddRemote => {
                if let Some(ticket) = self.repo.as_mut().and_then(|r| r.remotes.cancel()) {
                    self.executor.cancel(ticket);
                }
            }
            Action::RemoveRemote(name) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let label = tr!("remotes-remove-check-label", name = name.as_str());
                if let Some(params) = repo.remotes.start_remove(name) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = remotes::Op::Remove;
                    self.submit(op.command(), params, Request::Remote(generation, op));
                }
            }
            Action::ConfirmRemoveRemote => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((params, label)) = repo.remotes.confirm_remove() {
                    repo.write_label = Some(label);
                    let op = remotes::Op::Remove;
                    self.submit(op.command(), params, Request::Remote(generation, op));
                }
            }
            Action::CloseRemoteDialog => {
                if let Some(repo) = &mut self.repo {
                    repo.remotes.close();
                }
            }
            Action::DismissRemoteError => {
                if let Some(repo) = &mut self.repo {
                    repo.remotes.dismiss();
                }
            }
            _ => unreachable!("action dispatched to the wrong function"),
        }
    }

    /// Answer of `remote.add`/`remote.remove`: list of remotes, refs and journal from the
    /// answer, then the status and log reread (fetched or removed remote branches change the
    /// graph and the upstream of the current branch). A refused name or URL stays in the dialog.
    pub(crate) fn receive_remote(&mut self, generation: u64, op: remotes::Op, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo
            .remotes
            .receive(op, done.result, done.error_kind, done.error_detail)
        {
            remotes::Received::Inline | remotes::Received::Confirm => {
                self.error.clear();
                self.notice.clear();
            }
            remotes::Received::Done(outcome) => {
                log::info!("{}: {}", done.command, outcome.notice);
                repo.history = outcome.history.clone();
                let refs_read = outcome.refs.is_some();
                if let Some(refs) = outcome.refs {
                    repo.refs = refs;
                    repo.update_labels();
                }
                let remotes_read = outcome.remotes.is_some();
                self.notice = outcome.notice;
                self.error = outcome.warning.unwrap_or_default();
                if !refs_read {
                    self.submit("repo.refs", json!({}), Request::Refs(generation));
                }
                if !remotes_read {
                    self.request_remotes(generation);
                }
                self.reload_after_fetch(generation);
            }
            remotes::Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                } else {
                    self.request_remotes(generation);
                    self.submit("edit.history", json!({}), Request::History(generation));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_engine::{FetchFailure, MemoryBackend, MemoryOpener, Registry, Session};
    use std::sync::Arc;

    fn opened(repo: MemoryBackend) -> (Registry, Session) {
        let mut opener = MemoryOpener::new();
        opener.insert("r", repo);
        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(opener));
        registry
            .run("repo.open", &mut session, json!({ "path": "r" }))
            .unwrap();
        (registry, session)
    }

    /// Runs `command` like the executor and passes the answer to `receive`.
    fn run(
        registry: &Registry,
        session: &mut Session,
        b: &mut RemotesBox,
        op: Op,
        params: Value,
    ) -> Received {
        let result = registry.run(op.command(), session, params);
        let kind = result.as_ref().err().map(ErrorKind::of);
        let detail = result.as_ref().err().and_then(ErrorKind::detail);
        b.receive(op, result.map_err(|e| e.text()), kind, detail)
    }

    fn list(registry: &Registry, session: &mut Session, b: &mut RemotesBox) {
        let result = registry.run(LIST, session, json!({}));
        b.receive_list(result.map_err(|e| e.text()));
    }

    #[test]
    fn list_of_the_sample_repo() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = RemotesBox::default();
        assert!(b.list().is_none());
        assert!(b.names().is_empty());
        list(&registry, &mut session, &mut b);
        let remotes = b.list().unwrap();
        assert_eq!(remotes.len(), 1);
        let origin = &remotes[0];
        assert_eq!(origin.name, "origin");
        assert_eq!(origin.url_text(), "https://example.com/project.git");
        assert_eq!(origin.branches_text(), "2 branches");
        assert!(origin.default);
        assert_eq!(origin.badges()[0].0, "default");
        assert_eq!(b.names(), ["origin"]);
        assert_eq!(b.list_error(), None);

        b.receive_list(Err("broken".into()));
        assert_eq!(b.list_error(), Some("broken"));
        assert!(b.list().is_some(), "the previous list stays");
    }

    #[test]
    fn list_shows_remotes_without_branches_and_slashes() {
        let mut repo = MemoryBackend::sample();
        repo.add_remote("team/fork", "https://anna:secret@example.com/fork.git");
        let (registry, mut session) = opened(repo);
        let mut b = RemotesBox::default();
        list(&registry, &mut session, &mut b);
        let fork = b
            .list()
            .unwrap()
            .iter()
            .find(|r| r.name == "team/fork")
            .unwrap();
        assert_eq!(fork.branches_text(), "no branches");
        assert!(fork.credentials);
        assert!(!fork.url_text().contains("secret"), "{}", fork.url_text());
        assert!(fork.tooltip().contains("credentials"));
    }

    #[test]
    fn credentials_are_spotted_in_the_dialog() {
        assert!(looks_like_credentials("https://anna:pw@example.com/r.git"));
        assert!(looks_like_credentials("https://ghp_token@github.com/x/y"));
        assert!(looks_like_credentials("ssh://git:pw@host/r"));
        assert!(looks_like_credentials("remote-x::https://t@host/r"));
        assert!(!looks_like_credentials("ssh://git@host/r"));
        assert!(!looks_like_credentials("git@github.com:x/y.git"));
        assert!(!looks_like_credentials("https://example.com/a@b"));
        assert!(!looks_like_credentials("/local/path"));
    }

    #[test]
    fn add_with_fetch_on_the_memory_backend() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = RemotesBox::default();
        list(&registry, &mut session, &mut b);
        assert!(b.open_add());
        assert!(!b.open_add(), "one dialog at a time");
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        assert_eq!(dialog.fields.name, "", "origin is already taken");
        assert!(dialog.fields.fetch);
        assert!(!dialog.can_submit());
        assert_eq!(b.submit_add(), None);

        // A name already taken: the error stays in the dialog.
        b.set_fields(AddFields {
            name: "origin".into(),
            url: "https://example.com/other.git".into(),
            fetch: true,
        });
        let (params, _) = b.submit_add().unwrap();
        let r = run(&registry, &mut session, &mut b, Op::Add, params);
        assert!(matches!(r, Received::Inline), "{r:?}");
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        assert!(dialog.error.as_deref().unwrap().contains("already exists"));
        assert!(!dialog.sending);

        // URL with credentials: warning in the dialog and after.
        b.set_fields(AddFields {
            name: "fork".into(),
            url: " https://anna:pw@example.com/fork.git ".into(),
            fetch: true,
        });
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        assert!(dialog.credentials_warning().is_some());
        assert!(!dialog.url_masked(), "while typing it is visible");
        b.hide_url(true);
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        assert_eq!(dialog.shown_url(), "https://anna:***@example.com/fork.git");
        b.hide_url(false);
        let (params, label) = b.submit_add().unwrap();
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        // During the submit (and the fetch) the token is not on screen.
        assert!(dialog.url_masked());
        assert_eq!(dialog.shown_url(), "https://anna:***@example.com/fork.git");
        assert_eq!(label, "Adding remote “fork” and fetching");
        assert_eq!(params["url"], "https://anna:pw@example.com/fork.git");
        b.add_started(Ticket::for_test(3));
        assert!(b.running_fetch().is_some());
        assert_eq!(b.cancel(), Some(Ticket::for_test(3)));
        assert_eq!(b.cancel(), None, "only once");
        b.close();
        assert!(b.modal_open(), "it does not close during the submit");
        let r = run(&registry, &mut session, &mut b, Op::Add, params);
        let Received::Done(outcome) = r else {
            panic!("{r:?}")
        };
        assert!(
            outcome.notice.starts_with(
                "Remote “fork” added (https://anna:***@example.com/fork.git); fetch from “fork”"
            ),
            "{}",
            outcome.notice
        );
        assert!(outcome.notice.contains("“Undo” removes it"));
        assert!(
            outcome
                .notice
                .contains("warning: the URL contains credentials")
        );
        assert!(!outcome.notice.contains("pw@"));
        assert!(outcome.history.can_undo);
        assert!(b.dialog().is_none());
        assert_eq!(b.names(), ["fork", "origin"], "the list from the answer");
        assert!(b.failure().is_none());
    }

    #[test]
    fn add_with_fetch_downloads_the_branches() {
        // Removed origin and re-added with the same URL under another name: the fetch finds the
        // in-memory server branches again.
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = RemotesBox::default();
        let params = b.start_remove("origin".into()).unwrap();
        run(&registry, &mut session, &mut b, Op::Remove, params);
        let (params, _) = b.confirm_remove().unwrap();
        assert!(matches!(
            run(&registry, &mut session, &mut b, Op::Remove, params),
            Received::Done(_)
        ));
        b.open_add();
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        assert_eq!(dialog.fields.name, "origin", "proposed: it is gone");
        b.set_fields(AddFields {
            name: "upstream".into(),
            url: "https://example.com/project.git".into(),
            fetch: true,
        });
        let (params, _) = b.submit_add().unwrap();
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Add, params) else {
            panic!()
        };
        assert!(
            outcome.notice.contains("2 refs updated"),
            "{}",
            outcome.notice
        );
        let refs = outcome.refs.unwrap();
        assert!(refs.iter().any(|r| r.name == "refs/remotes/upstream/main"));
        assert_eq!(b.list().unwrap()[0].branches, 2);
    }

    #[test]
    fn add_with_a_failed_fetch_keeps_the_remote_and_explains() {
        let mut repo = MemoryBackend::sample();
        // The in-memory server of an already seen URL: rejects the credentials.
        repo.add_remote("old", "https://example.com/private.git");
        repo.fail_fetch(
            "old",
            Some(FetchFailure::Auth("fatal: Authentication failed".into())),
        );
        let (registry, mut session) = opened(repo);
        let mut b = RemotesBox::default();
        let params = b.start_remove("old".into()).unwrap();
        run(&registry, &mut session, &mut b, Op::Remove, params);
        let (params, _) = b.confirm_remove().unwrap();
        run(&registry, &mut session, &mut b, Op::Remove, params);

        b.open_add();
        b.set_fields(AddFields {
            name: "private".into(),
            url: "https://example.com/private.git".into(),
            fetch: true,
        });
        let (params, _) = b.submit_add().unwrap();
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Add, params) else {
            panic!()
        };
        assert!(
            outcome.notice.contains("fetch failed: authentication"),
            "{}",
            outcome.notice
        );
        let failure = b.failure().unwrap();
        assert_eq!(failure.title, "Authentication failed");
        assert!(failure.explanation.contains("added anyway"));
        assert!(!failure.explanation.contains("Nothing was changed"));
        assert!(b.names().contains(&"private".to_owned()));
        b.dismiss();
        assert!(b.failure().is_none());
    }

    #[test]
    fn remove_asks_for_confirmation_and_undo_restores() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = RemotesBox::default();
        list(&registry, &mut session, &mut b);
        let params = b.start_remove("origin".into()).unwrap();
        assert_eq!(params, json!({ "name": "origin" }), "first without confirm");
        assert!(!b.modal_open(), "being checked: no dialog");
        assert_eq!(b.start_remove("origin".into()), None);
        let r = run(&registry, &mut session, &mut b, Op::Remove, params);
        assert!(matches!(r, Received::Confirm), "{r:?}");
        let Some(Dialog::Remove(remove)) = b.dialog() else {
            panic!()
        };
        let Phase::Confirm { reason } = &remove.phase else {
            panic!()
        };
        assert!(reason.contains("main"), "{reason}");
        // Nothing changed until confirmed.
        list(&registry, &mut session, &mut b);
        assert_eq!(b.names(), ["origin"]);

        // "Don't remove" closes without sending anything.
        b.close();
        assert!(b.dialog().is_none());
        let params = b.start_remove("origin".into()).unwrap();
        run(&registry, &mut session, &mut b, Op::Remove, params);
        let (params, label) = b.confirm_remove().unwrap();
        assert_eq!(params, json!({ "name": "origin", "confirm": true }));
        assert_eq!(label, "Removing remote “origin”");
        assert_eq!(b.confirm_remove(), None, "only once");
        let r = run(&registry, &mut session, &mut b, Op::Remove, params);
        let Received::Done(outcome) = r else {
            panic!("{r:?}")
        };
        assert!(
            outcome
                .notice
                .starts_with("Remote “origin” removed (deleted 2 remote-tracking branches); no upstream anymore: “"),
            "{}",
            outcome.notice
        );
        assert!(outcome.notice.contains("“Undo” restores it"));
        assert!(b.names().is_empty());
        let refs = outcome.refs.unwrap();
        assert!(!refs.iter().any(|r| r.name.starts_with("refs/remotes/")));
        assert!(refs.iter().all(|r| r.upstream.is_none()));

        registry.run("edit.undo", &mut session, json!({})).unwrap();
        list(&registry, &mut session, &mut b);
        assert_eq!(b.names(), ["origin"]);
        assert_eq!(b.list().unwrap()[0].branches, 2);
    }

    #[test]
    fn remove_of_an_unknown_remote_is_refused() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = RemotesBox::default();
        let params = b.start_remove("nobody".into()).unwrap();
        let Received::Failed { message, reload } =
            run(&registry, &mut session, &mut b, Op::Remove, params)
        else {
            panic!()
        };
        assert!(message.starts_with("Removing remote “nobody” not possible"));
        assert!(!reload);
        assert!(b.dialog().is_none());
    }

    #[test]
    fn ahead_behind_text_and_tooltip() {
        let up = |ahead, behind| Upstream {
            name: "refs/remotes/origin/main".to_owned(),
            ahead,
            behind,
        };
        assert_eq!(
            ahead_behind(&up(Some(0), Some(0))),
            (String::new(), "Up to date with origin/main".to_owned())
        );
        assert_eq!(
            ahead_behind(&up(Some(2), Some(1))),
            (
                "↑2 ↓1".to_owned(),
                "2 commits to push, 1 commit to pull compared to origin/main".to_owned()
            )
        );
        assert_eq!(ahead_behind(&up(Some(0), Some(3))).0, "↓3");
        let (short, long) = ahead_behind(&up(None, None));
        assert_eq!(short, "upstream gone");
        assert!(long.contains("origin/main no longer exists"), "{long}");
        // The status gives the short name.
        let status = Upstream {
            name: "origin/main".to_owned(),
            ahead: Some(1),
            behind: Some(0),
        };
        assert_eq!(
            ahead_behind(&status),
            (
                "↑1".to_owned(),
                "1 commit to push compared to origin/main".to_owned()
            )
        );
    }

    #[test]
    fn ahead_behind_of_the_sample_after_fetch() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        registry
            .run(fetch::COMMAND, &mut session, json!({}))
            .unwrap();
        let refs: Vec<RefInfo> =
            serde_json::from_value(registry.run("repo.refs", &mut session, json!({})).unwrap())
                .unwrap();
        let text = |name: &str| {
            let info = refs.iter().find(|r| r.name == name).unwrap();
            ahead_behind(info.upstream.as_ref().unwrap()).0
        };
        assert_eq!(text("refs/heads/main"), "↑1");
        assert_eq!(text("refs/heads/feature/graph"), "↓1");
    }
}
