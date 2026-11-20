//! The git identity (S65): `user.name` and `user.email` in the "Git" section of the settings
//! window, read with `config.identity` and saved with `config.set_identity`, and the offer to
//! set them when a command could not create a commit without them.
//!
//! These values are git's configuration, not the app's settings: nothing is written until
//! "Save", which goes through the `Registry` like every other write, on the tab shown (its repo
//! is "this repository"). They are read when the section is shown and after a save that
//! failed, never per frame. Data only, no egui: tested with unit tests.
//!
//! A command that fails for a missing identity offers "Set name and email…" where its error is
//! shown: under the commit box ([`crate::commit::CommitError::identity`]), in its failure
//! dialog ([`Failure::identity`]: merge, cherry-pick, revert, pull) or, for the commands whose
//! error stays in their own dialog or in the status bar (the "Continue" of an operation, the
//! rebase, a stash, an annotated tag), in a failure dialog of its own ([`failure`]).

use crate::executor::{Done, Ticket};
use crate::fetch::Failure;
use crate::{Request, Tab};
use project_engine::{
    ConfigScope, Identity, IdentityKey, IdentityPair, IdentityScope, SetIdentityParams,
    SetIdentityResult,
};
use project_i18n::tr;
use serde_json::{Value, json};

const READ: &str = "config.identity";
const SAVE: &str = "config.set_identity";

/// The actions of the "Git" section, applied by [`App`](crate::App).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// "For all repositories" or "Only for this repository".
    Scope(IdentityScope),
    /// The text of the name field.
    Name(String),
    /// The text of the email field.
    Email(String),
    /// "Save": `config.set_identity` with the fields that changed.
    Save,
    /// "Retry" after a read that failed.
    Reload,
    /// The keyboard went to the field [`IdentityBox::focus`] asked for.
    Focused,
}

/// Whether `message` (the text of a failed command, which contains git's) says that git has no
/// usable name or email: the engine decides ([`project_engine::is_missing_identity`]).
pub(crate) fn missing(message: &str) -> bool {
    project_engine::is_missing_identity(message)
}

/// The failure dialog of a command stopped by a missing identity whose own error is not shown
/// in a failure dialog: what happened, git's text and "Set name and email…".
pub(crate) fn failure(git: Option<String>) -> Failure {
    Failure {
        title: tr!("common-identity-title"),
        explanation: tr!("common-identity-explanation"),
        git,
        stash: false,
        retry: None,
        identity: true,
    }
}

/// Whose values the section shows: the tab the commands run on and the repo it had open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Context {
    /// [`Tab::id`].
    pub tab: u64,
    /// The path of the open repo: "this repository".
    pub repo: Option<String>,
}

/// A `config.set_identity` in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Saving {
    tab: u64,
    ticket: Ticket,
    scope: IdentityScope,
}

/// What an answer was, for the status bar of its tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Received {
    /// Not an answer the section waits for (the window was closed, another tab is shown).
    Nothing,
    /// `config.identity` answered: shown in the section, errors included.
    Read,
    /// Saved: the notice, with the values replaced.
    Saved(String),
    /// Not saved: the error.
    Failed(String),
}

/// The "Git" section: the values read, the form and the read or save in progress.
#[derive(Debug)]
pub(crate) struct IdentityBox {
    /// Whose values these are; `None`: nothing asked yet.
    context: Option<Context>,
    /// The section was shown again, or a save failed: the values are to be read again.
    stale: bool,
    reading: Option<Ticket>,
    /// The values last read (or returned by a save) for `context`.
    identity: Option<Identity>,
    read_error: Option<String>,
    saving: Option<Saving>,
    /// How the last save ended, shown under the button: the notice or the error.
    outcome: Option<Result<String, String>>,
    scope: IdentityScope,
    name: String,
    email: String,
    /// The keyboard is to go to the first empty field ("Set name and email…").
    focus: bool,
}

impl Default for IdentityBox {
    fn default() -> Self {
        Self {
            context: None,
            stale: false,
            reading: None,
            identity: None,
            read_error: None,
            saving: None,
            outcome: None,
            scope: IdentityScope::Global,
            name: String::new(),
            email: String::new(),
            focus: false,
        }
    }
}

/// The text of a field for a value of the configuration (`None`: not set).
fn field(value: Option<&str>) -> &str {
    value.unwrap_or("")
}

/// Whether the text of a field is to be written over `current`. A key set to an empty value
/// counts as changed even with an empty field: saving removes it, and the scope below it
/// applies again.
fn changed(text: &str, current: Option<&str>) -> bool {
    match current {
        None => !text.is_empty(),
        Some(value) => text != value || value.is_empty(),
    }
}

impl IdentityBox {
    /// The section is shown (the window opened on it, or it was chosen): the values are read
    /// again. What was typed in a field stays.
    pub fn show(&mut self) {
        self.stale = true;
    }

    /// [`IdentityBox::show`] from "Set name and email…": the keyboard goes to the first empty
    /// field.
    pub fn show_focused(&mut self) {
        self.show();
        self.focus = true;
    }

    /// The window closed: what was typed and not saved is dropped. A save in progress goes on
    /// and its answer still reaches the status bar.
    pub fn close(&mut self) {
        *self = Self {
            saving: self.saving,
            ..Self::default()
        };
    }

    /// True if `config.identity` is to be sent for `context`: the section was shown again, or
    /// it shows the values of another tab or repo. Never while a read or a save is in progress.
    pub fn wants(&self, context: &Context) -> bool {
        self.reading.is_none()
            && self.saving.is_none()
            && (self.stale || self.context.as_ref() != Some(context))
    }

    /// `config.identity` was sent for `context`.
    pub fn reading(&mut self, context: Context, ticket: Ticket) {
        if self.context.as_ref() != Some(&context) {
            // Another tab or repo: nothing of the old one applies, the form starts again.
            self.identity = None;
            self.outcome = None;
            self.name.clear();
            self.email.clear();
        }
        self.context = Some(context);
        self.stale = false;
        self.read_error = None;
        self.reading = Some(ticket);
    }

    /// The values read, once there are some.
    pub fn identity(&self) -> Option<&Identity> {
        self.identity.as_ref()
    }

    /// The path of the repo the values are of: with one, "Only for this repository" is offered.
    pub fn repo(&self) -> Option<&str> {
        self.context.as_ref()?.repo.as_deref()
    }

    pub fn is_reading(&self) -> bool {
        self.reading.is_some()
    }

    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// The error of the last read, if it failed.
    pub fn read_error(&self) -> Option<&str> {
        self.read_error.as_deref()
    }

    /// How the last save ended: the notice or the error.
    pub fn outcome(&self) -> Option<&Result<String, String>> {
        self.outcome.as_ref()
    }

    pub fn scope(&self) -> IdentityScope {
        self.scope
    }

    /// The text of the field of `key`.
    pub fn text(&self, key: IdentityKey) -> &str {
        match key {
            IdentityKey::Name => &self.name,
            IdentityKey::Email => &self.email,
        }
    }

    /// The field the keyboard is to go to, once: the first empty one, or the name.
    pub fn focus(&self) -> Option<IdentityKey> {
        let empty = |key: &IdentityKey| self.text(*key).is_empty();
        self.focus.then(|| {
            IdentityKey::ALL
                .into_iter()
                .find(empty)
                .unwrap_or(IdentityKey::Name)
        })
    }

    /// The value of `key` a field left empty would fall back to: with "Only for this
    /// repository", the one for all repositories.
    pub fn inherited(&self, key: IdentityKey) -> Option<&str> {
        let identity = self.identity.as_ref()?;
        (self.scope == IdentityScope::Local)
            .then(|| identity.global.get(key))
            .flatten()
            .filter(|value| !value.is_empty())
    }

    /// The fields that differ from the values of the chosen scope.
    fn changes(&self) -> Vec<IdentityKey> {
        let Some(identity) = &self.identity else {
            return Vec::new();
        };
        let current = identity.of(self.scope);
        IdentityKey::ALL
            .into_iter()
            .filter(|key| changed(self.text(*key), current.get(*key)))
            .collect()
    }

    /// True if "Save" has something to write and nothing is in progress.
    pub fn can_save(&self) -> bool {
        self.reading.is_none() && self.saving.is_none() && !self.changes().is_empty()
    }

    /// The parameters of `config.set_identity`: the chosen scope and only the fields that
    /// changed (an empty one removes the key). `None` if there is nothing to save.
    pub fn save(&self) -> Option<Value> {
        if !self.can_save() {
            return None;
        }
        let changes = self.changes();
        let given = |key| changes.contains(&key).then(|| self.text(key).to_owned());
        Some(crate::params(SetIdentityParams {
            scope: self.scope,
            name: given(IdentityKey::Name),
            email: given(IdentityKey::Email),
        }))
    }

    /// `config.set_identity` was sent on the tab `tab`.
    pub fn sending(&mut self, tab: u64, ticket: Ticket) {
        self.outcome = None;
        self.saving = Some(Saving {
            tab,
            ticket,
            scope: self.scope,
        });
    }

    /// Applies an action of the form; "Save" and "Retry" are the caller's.
    pub fn apply(&mut self, action: Action) {
        match action {
            Action::Scope(scope) => self.set_scope(scope),
            Action::Name(name) => self.name = name,
            Action::Email(email) => self.email = email,
            Action::Reload => self.stale = true,
            Action::Focused => self.focus = false,
            Action::Save => {}
        }
    }

    /// Chooses where "Save" writes. A field still as the scope left shows the value of the new
    /// one; a field that was edited keeps what was typed.
    fn set_scope(&mut self, scope: IdentityScope) {
        if scope == self.scope || (scope == IdentityScope::Local && self.repo().is_none()) {
            return;
        }
        if let Some(identity) = &self.identity {
            let (old, new) = (identity.of(self.scope).clone(), identity.of(scope).clone());
            self.follow(&old, &new);
        }
        // The notice of the last save was about the other scope.
        self.outcome = self.outcome.take().filter(Result::is_err);
        self.scope = scope;
    }

    /// The fields that still show `old` now show `new`.
    fn follow(&mut self, old: &IdentityPair, new: &IdentityPair) {
        for key in IdentityKey::ALL {
            let slot = match key {
                IdentityKey::Name => &mut self.name,
                IdentityKey::Email => &mut self.email,
            };
            if slot.as_str() == field(old.get(key)) {
                field(new.get(key)).clone_into(slot);
            }
        }
    }

    /// The values just read or returned by a save.
    fn loaded(&mut self, new: Identity) {
        match self.identity.take() {
            // The first values of this tab: the form starts on the repo's own values if it
            // has some, otherwise on those for all repositories.
            None => {
                let own = new.local.name.is_some() || new.local.email.is_some();
                self.scope = if self.repo().is_some() && own {
                    IdentityScope::Local
                } else {
                    IdentityScope::Global
                };
                let pair = new.of(self.scope);
                field(pair.name.as_deref()).clone_into(&mut self.name);
                field(pair.email.as_deref()).clone_into(&mut self.email);
            }
            // Read again: a field the user changed keeps what was typed.
            Some(old) => {
                let (old, new) = (old.of(self.scope).clone(), new.of(self.scope).clone());
                self.follow(&old, &new);
            }
        }
        self.identity = Some(new);
    }

    /// The answer `done` of a command sent on the tab `tab` for this section.
    pub fn receive(&mut self, tab: u64, done: Done) -> Received {
        let read = self.context.as_ref().is_some_and(|c| c.tab == tab)
            && self.reading == Some(done.ticket)
            && done.command == READ;
        if read {
            self.reading = None;
            let result = done.result;
            match result.and_then(|value| crate::decode::<Identity>(READ, value)) {
                Ok(identity) => self.loaded(identity),
                Err(error) => self.read_error = Some(error),
            }
            return Received::Read;
        }
        let Some(saving) = self
            .saving
            .filter(|s| s.tab == tab && s.ticket == done.ticket && done.command == SAVE)
        else {
            return Received::Nothing;
        };
        self.saving = None;
        let result = done.result;
        match result.and_then(|value| crate::decode::<SetIdentityResult>(SAVE, value)) {
            Ok(result) => {
                let notice = saved_text(saving.scope, &result);
                // Only for the form that sent it: after the window closed there is none.
                if self.context.as_ref().is_some_and(|c| c.tab == tab) {
                    self.outcome = Some(Ok(notice.clone()));
                    self.loaded(result.identity);
                }
                Received::Saved(notice)
            }
            Err(error) => {
                if self.context.as_ref().is_some_and(|c| c.tab == tab) {
                    self.outcome = Some(Err(error.clone()));
                    // The first key may have been written before the second one failed.
                    self.stale = true;
                }
                Received::Failed(tr!("settings-identity-save-failed", error = &error))
            }
        }
    }
}

/// A value replaced by a save, for the notice: quoted, or "not set".
fn shown(value: Option<&str>) -> String {
    match value {
        Some(value) => tr!("settings-identity-quoted", value = value),
        None => tr!("settings-identity-unset"),
    }
}

/// The notice of a save: where it was written and the values it replaced (the configuration
/// is not in the undo journal: this is where they can be read to put them back).
fn saved_text(scope: IdentityScope, result: &SetIdentityResult) -> String {
    if !result.changed {
        return tr!("settings-identity-unchanged");
    }
    let name = shown(result.previous.name.as_deref());
    let email = shown(result.previous.email.as_deref());
    match scope {
        IdentityScope::Global => tr!("settings-identity-saved-global", name = name, email = email),
        IdentityScope::Local => tr!("settings-identity-saved-local", name = name, email = email),
    }
}

/// Where the value git uses comes from, for the "Git uses" rows.
pub(crate) fn source_text(scope: ConfigScope) -> String {
    match scope {
        ConfigScope::Local => tr!("settings-identity-from-local"),
        ConfigScope::Global => tr!("settings-identity-from-global"),
        ConfigScope::System => tr!("settings-identity-from-system"),
        ConfigScope::Other => tr!("settings-identity-from-other"),
    }
}

/// The value of `key` for all repositories that the repo's own value hides, if both are set.
pub(crate) fn hidden(identity: &Identity, key: IdentityKey) -> Option<&str> {
    identity.local.get(key).and(identity.global.get(key))
}

/// The environment variables that win over the configuration (`GIT_AUTHOR_*`,
/// `GIT_COMMITTER_*`), and whether `EMAIL` stands in for an email that is not configured.
pub(crate) fn environment(identity: &Identity) -> (Vec<&str>, bool) {
    let (fallback, wins): (Vec<&str>, Vec<&str>) = identity
        .env
        .iter()
        .map(String::as_str)
        .partition(|name| *name == "EMAIL");
    (wins, !fallback.is_empty())
}

impl Tab {
    /// Sends `config.identity` for the "Git" section: the answer waits in `identity_done`.
    pub(crate) fn read_identity(&mut self) -> Ticket {
        self.submit(READ, json!({}), Request::Identity)
    }

    /// Sends `config.set_identity` with the parameters of [`IdentityBox::save`].
    pub(crate) fn save_identity(&mut self, params: Value) -> Ticket {
        self.submit(SAVE, params, Request::Identity)
    }

    /// The identity was saved: the offers to set it and the commit error it explains go away.
    /// The commit box is as it was: the message and the staged files are not touched, and the
    /// commit is not sent again.
    pub(crate) fn identity_saved(&mut self) {
        if let Some(repo) = &mut self.repo {
            repo.identity = None;
            repo.commit.dismiss_identity_error();
        }
    }

    /// A command of the repo of `generation` failed because git has no name or email, and its
    /// error is not in a failure dialog that offers to set them: the dialog of [`failure`]
    /// does, with git's text.
    pub(crate) fn offer_identity(&mut self, generation: u64, git: Option<String>) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        if !repo.offers_identity() {
            repo.identity = Some(failure(git));
        }
    }
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;
