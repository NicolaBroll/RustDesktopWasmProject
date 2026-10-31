//! "Edit URL…" and "Rename…" of a remote (S59): `remote.set_url` and `remote.rename`, from the
//! remote's "⋯" menu in the refs column. Data only, no egui: tested with unit tests. A child of
//! [`crate::remotes`], whose single dialog slot it shares.
//!
//! The URLs from `remote.list` are masked: a masked URL is never put in a field (it would be
//! written back with `***` in place of the password). A URL with credentials leaves its field empty
//! with the masked one as hint; an empty field, or one left as prefilled, is not sent. The fetch URL
//! and the separate push URL are two keys, so two `remote.set_url` in a row (each one its own entry
//! in the journal); the second starts only after the first succeeded.

use super::*;
use project_engine::{
    RemoteRenameParams, RemoteRenameResult, RemoteSetUrlParams, RemoteSetUrlResult,
};

/// What the edit dialog changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Fetch URL and, optionally, a separate push URL (`remote.set_url`).
    Url,
    /// The name (`remote.rename`).
    Rename,
}

/// The fields of the edit dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditFields {
    /// The fetch URL (`remote.<name>.url`).
    pub url: String,
    /// The separate push URL (`remote.<name>.pushurl`); empty = left as it is.
    pub push_url: String,
    /// The new name.
    pub new_name: String,
}

/// How git reaches the server: it decides which credentials are used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Https,
    Ssh,
}

/// The transport of `url`: `http(s)://` or SSH (`ssh://`, scp-like `user@host:path`); `None` for
/// local paths, `file://`, `git://` and helpers.
pub fn transport(url: &str) -> Option<Transport> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return Some(Transport::Https);
    }
    if ["ssh://", "git+ssh://", "ssh+git://"]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        return Some(Transport::Ssh);
    }
    if url.contains("://") {
        return None;
    }
    // scp-like: `host:path` with no `/` before the `:`; a single letter is a Windows drive.
    let (host, _) = url.split_once(':')?;
    (host.len() > 1 && !host.contains('/') && !host.contains('\\')).then_some(Transport::Ssh)
}

/// The edit dialog of a remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditDialog {
    pub remote: String,
    pub kind: EditKind,
    pub fields: EditFields,
    /// The fetch URL as `remote.list` gives it (masked).
    pub current_url: Option<String>,
    /// The push URLs as `remote.list` gives them (masked).
    pub current_push: Vec<String>,
    /// The values the fields were opened with: a field equal to these is not sent.
    prefilled_url: String,
    prefilled_push: String,
    /// The engine error from the last submit, shown in the dialog.
    pub error: Option<String>,
    /// A submit is running.
    pub sending: bool,
    /// The `remote.set_url` running is the one of the push URL.
    running_push: bool,
    /// The `remote.set_url` (URL, push) still to send after the running one.
    queue: Vec<(String, bool)>,
    /// The notices of the steps already done in this submit.
    done: Vec<String>,
}

/// A URL that can be put in a field: shown as it is (no credentials, so not masked).
fn editable(url: Option<&String>) -> String {
    url.filter(|u| !looks_like_credentials(u))
        .cloned()
        .unwrap_or_default()
}

impl EditDialog {
    fn new(remote: &RemoteListEntry, kind: EditKind) -> Self {
        let prefilled_url = editable(remote.url.as_ref());
        let prefilled_push = if remote.push_urls.len() == 1 {
            editable(remote.push_urls.first())
        } else {
            String::new()
        };
        EditDialog {
            remote: remote.name.clone(),
            kind,
            fields: EditFields {
                url: prefilled_url.clone(),
                push_url: prefilled_push.clone(),
                new_name: remote.name.clone(),
            },
            current_url: remote.url.clone(),
            current_push: remote.push_urls.clone(),
            prefilled_url,
            prefilled_push,
            error: None,
            sending: false,
            running_push: false,
            queue: Vec::new(),
            done: Vec::new(),
        }
    }

    /// The hint of the fetch URL field: the current (masked) URL when it could not be prefilled.
    pub fn url_hint(&self) -> String {
        match &self.current_url {
            Some(url) if self.prefilled_url.is_empty() => url.clone(),
            _ => tr!("remotes-add-url-hint"),
        }
    }

    /// The hint of the push URL field: the current (masked) push URL(s) when not prefilled.
    pub fn push_hint(&self) -> String {
        if self.current_push.is_empty() {
            tr!("remotes-edit-push-url-hint")
        } else if self.prefilled_push.is_empty() {
            self.current_push.join(", ")
        } else {
            String::new()
        }
    }

    /// True if a field holds a masked URL left empty: typing replaces it, empty leaves it.
    pub fn masked_note(&self) -> Option<String> {
        let masked = (self.current_url.is_some() && self.prefilled_url.is_empty())
            || (!self.current_push.is_empty() && self.prefilled_push.is_empty());
        masked.then(|| tr!("remotes-edit-masked-note"))
    }

    /// The `remote.set_url` to send, in order: the fields typed and different from the prefill.
    pub fn steps(&self) -> Vec<(String, bool)> {
        let mut steps = Vec::new();
        for (value, prefilled, push) in [
            (&self.fields.url, &self.prefilled_url, false),
            (&self.fields.push_url, &self.prefilled_push, true),
        ] {
            let value = value.trim();
            if !value.is_empty() && value != prefilled.trim() {
                steps.push((value.to_owned(), push));
            }
        }
        steps
    }

    /// True if the button is enabled. The rest is checked by the engine.
    pub fn can_submit(&self) -> bool {
        !self.sending
            && match self.kind {
                EditKind::Url => !self.steps().is_empty(),
                EditKind::Rename => {
                    let name = self.fields.new_name.trim();
                    !name.is_empty() && name != self.remote
                }
            }
    }

    /// The one-line note when the fetch URL changes between HTTPS and SSH.
    pub fn transport_note(&self) -> Option<String> {
        if self.kind != EditKind::Url {
            return None;
        }
        let new = transport(&self.fields.url)?;
        let old = transport(self.current_url.as_deref()?)?;
        match (old, new) {
            (Transport::Https, Transport::Ssh) => Some(tr!("remotes-edit-to-ssh")),
            (Transport::Ssh, Transport::Https) => Some(tr!("remotes-edit-to-https")),
            _ => None,
        }
    }

    /// The warning when a typed URL contains credentials.
    pub fn credentials_warning(&self) -> Option<String> {
        (self.kind == EditKind::Url
            && (looks_like_credentials(&self.fields.url)
                || looks_like_credentials(&self.fields.push_url)))
        .then(|| tr!("remotes-credentials-warning"))
    }

    fn label(&self, push: bool) -> String {
        match (self.kind, push) {
            (EditKind::Rename, _) => tr!(
                "remotes-rename-label",
                name = &self.remote,
                new = self.fields.new_name.trim()
            ),
            (EditKind::Url, false) => tr!("remotes-set-url-label", name = &self.remote),
            (EditKind::Url, true) => tr!("remotes-set-push-url-label", name = &self.remote),
        }
    }

    fn set_url_params(&mut self, (url, push): (String, bool)) -> (Op, Value, String) {
        self.running_push = push;
        let params = params(RemoteSetUrlParams {
            name: self.remote.clone(),
            url,
            push,
        });
        (Op::SetUrl, params, self.label(push))
    }
}

impl RemotesBox {
    fn entry(&self, name: &str) -> Option<&RemoteListEntry> {
        self.list.iter().flatten().find(|r| r.name == name)
    }

    /// Opens "Edit URL…" or "Rename…" of the remote `name` (it must be in the list).
    pub fn open_edit(&mut self, name: &str, kind: EditKind) -> bool {
        if self.dialog.is_some() {
            return false;
        }
        let Some(remote) = self.entry(name) else {
            return false;
        };
        self.dialog = Some(Dialog::Edit(EditDialog::new(remote, kind)));
        true
    }

    /// The fields of the edit dialog changed.
    pub fn set_edit_fields(&mut self, fields: EditFields) {
        if let Some(Dialog::Edit(dialog)) = &mut self.dialog
            && !dialog.sending
        {
            dialog.fields = fields;
        }
    }

    /// Submit of the edit dialog: command, parameters and description of the first step.
    pub fn submit_edit(&mut self) -> Option<(Op, Value, String)> {
        let Some(Dialog::Edit(dialog)) = &mut self.dialog else {
            return None;
        };
        if !dialog.can_submit() {
            return None;
        }
        dialog.error = None;
        dialog.sending = true;
        dialog.done.clear();
        match dialog.kind {
            EditKind::Rename => {
                let params = params(RemoteRenameParams {
                    name: dialog.remote.clone(),
                    new_name: dialog.fields.new_name.trim().to_owned(),
                });
                Some((Op::Rename, params, dialog.label(false)))
            }
            EditKind::Url => {
                let mut steps = dialog.steps();
                let first = steps.remove(0);
                dialog.queue = steps;
                Some(dialog.set_url_params(first))
            }
        }
    }

    pub(super) fn receive_edit(
        &mut self,
        op: Op,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
    ) -> Received {
        let dialog = match &mut self.dialog {
            Some(Dialog::Edit(dialog))
                if dialog.sending && (op == Op::Rename) == (dialog.kind == EditKind::Rename) =>
            {
                dialog
            }
            _ => return unexpected(op),
        };
        match result {
            // Invalid URL or name, name taken, unknown remote, operation in progress: nothing
            // written by this step, fixed in the dialog (with what the steps before did).
            Err(e) if matches!(kind, Some(ErrorKind::InvalidParams | ErrorKind::NotAllowed)) => {
                dialog.sending = false;
                dialog.queue.clear();
                let mut parts = std::mem::take(&mut dialog.done);
                parts.push(e);
                dialog.error = Some(parts.join("\n"));
                Received::Inline
            }
            Err(e) => {
                let (remote, push) = (dialog.remote.clone(), dialog.running_push);
                self.dialog = None;
                let message = match op {
                    Op::Rename => tr!("remotes-rename-failed", name = remote, error = e),
                    _ if push => tr!("remotes-set-push-url-failed", name = remote, error = e),
                    _ => tr!("remotes-set-url-failed", name = remote, error = e),
                };
                Received::Failed {
                    message,
                    reload: true,
                }
            }
            Ok(value) if op == Op::Rename => {
                self.dialog = None;
                match crate::decode::<RemoteRenameResult>(op.command(), value) {
                    Ok(result) => Received::Done(Box::new(rename_outcome(result))),
                    Err(message) => Received::Failed {
                        message,
                        reload: true,
                    },
                }
            }
            Ok(value) => {
                let result = match crate::decode::<RemoteSetUrlResult>(op.command(), value) {
                    Ok(result) => result,
                    Err(message) => {
                        self.dialog = None;
                        return Received::Failed {
                            message,
                            reload: true,
                        };
                    }
                };
                let push = dialog.running_push;
                // Written: a resubmit after an error in the next step does not send it again.
                if push {
                    dialog.prefilled_push = dialog.fields.push_url.trim().to_owned();
                } else {
                    dialog.prefilled_url = dialog.fields.url.trim().to_owned();
                }
                let mut outcome = set_url_outcome(result, push);
                if dialog.queue.is_empty() {
                    let mut parts = std::mem::take(&mut dialog.done);
                    parts.push(outcome.notice);
                    outcome.notice = parts.join(". ");
                    self.dialog = None;
                    Received::Done(Box::new(outcome))
                } else {
                    dialog.done.push(outcome.notice.clone());
                    let next = dialog.queue.remove(0);
                    let (op, params, label) = dialog.set_url_params(next);
                    Received::Step {
                        outcome: Box::new(outcome),
                        op,
                        params,
                        label,
                    }
                }
            }
        }
    }
}

/// The outcome of `remote.set_url` (of the push URL if `push`).
fn set_url_outcome(r: RemoteSetUrlResult, push: bool) -> Outcome {
    let name = &r.remote;
    let url = if push { &r.push_url } else { &r.url };
    let url = url.clone().unwrap_or_else(|| tr!("remotes-no-url"));
    let mut notice = match (r.changed, push) {
        (true, false) => tr!("remotes-url-set", name = name, url = url),
        (true, true) => tr!("remotes-push-url-set", name = name, url = url),
        (false, false) => tr!("remotes-url-unchanged", name = name, url = url),
        (false, true) => tr!("remotes-push-url-unchanged", name = name, url = url),
    };
    if r.changed {
        notice.push_str(&tr!("remotes-url-set-undo"));
    }
    notice.push_str(&messages_text(&r.warnings));
    Outcome {
        notice,
        refs: r.refs,
        remotes: r.remotes,
        warning: reread_warning(&tr!("remotes-url-set-short"), r.refs_error, r.remotes_error),
        history: Some(r.journal.into()),
    }
}

/// The outcome of `remote.rename`.
fn rename_outcome(r: RemoteRenameResult) -> Outcome {
    let mut notice = tr!("remotes-renamed", name = &r.from, new = &r.to);
    if !r.moved_refs.is_empty() {
        notice.push_str(&tr!("remotes-renamed-refs", n = r.moved_refs.len()));
    }
    if !r.upstreams.is_empty() {
        notice.push_str(&tr!(
            "remotes-renamed-upstreams",
            names = quoted(&r.upstreams),
            new = &r.to
        ));
    }
    notice.push_str(&tr!("remotes-renamed-undo"));
    notice.push_str(&messages_text(&r.warnings));
    Outcome {
        notice,
        refs: r.refs,
        remotes: r.remotes,
        warning: reread_warning(&tr!("remotes-renamed-short"), r.refs_error, r.remotes_error),
        history: Some(r.journal.into()),
    }
}

#[cfg(test)]
#[path = "remote_edit_tests.rs"]
mod tests;
