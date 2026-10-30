//! Credential prompts (S52): git and ssh ask for a user name, password, passphrase or host key
//! confirmation during fetch, pull, push and clone, and the app asks the user.

use project_engine::{CredentialPrompter, CredentialRequest};

/// The desktop app's [`CredentialPrompter`], installed on the `CliOpener` by `apps/project`.
///
/// For now a stub that cancels every question (the command ends with
/// [`EngineError::AuthCancelled`](project_engine::EngineError::AuthCancelled)): the dialog comes
/// in the egui-ui part of S52. `ask` is called on the worker thread of the command and must
/// block until the user answers (`None` = cancelled); the answer is a secret, never logged or
/// kept (masked field for [`CredentialKind::is_secret`](project_engine::CredentialKind::is_secret)).
#[derive(Debug, Default)]
pub struct CredentialDialog {
    /// The state the dialog will need (channel to the UI, `egui::Context` to wake it).
    _todo: (),
}

impl CredentialDialog {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialPrompter for CredentialDialog {
    fn ask(&self, _request: CredentialRequest) -> Option<String> {
        None
    }
}
