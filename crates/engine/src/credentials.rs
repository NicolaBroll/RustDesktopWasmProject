//! Credential prompts (S52): git and ssh ask for a user name, a password, a key passphrase or a
//! host key confirmation, and the app asks the user.
//!
//! The network commands run git with no terminal (`impl-docs/ARCHITECTURE-git.md` §5): without a
//! credential helper or an ssh-agent they could only fail. A backend that supports prompts (the
//! `CliBackend` with an askpass bridge) hands each question to a [`CredentialPrompter`] set by
//! the app; without one (MCP server, tests) nothing is ever asked and the command fails with
//! [`EngineError::Auth`](crate::EngineError::Auth) as before.
//!
//! The answers are secrets: they go straight back to git and are never logged, stored, put in
//! errors, progress, journal or `Debug` output. git's own credential helper stores them after a
//! successful command, as in a terminal.

/// What git or ssh asks for, recognized from its prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialKind {
    /// A user name (git: `Username for 'https://host': `). Not secret.
    Username,
    /// A password or token (git: `Password for 'https://user@host': `, ssh: `user@host's
    /// password: `). Secret: the field must not show it.
    Password,
    /// The passphrase of an ssh key (`Enter passphrase for key '/home/u/.ssh/id_ed25519': `).
    /// Secret.
    Passphrase,
    /// ssh asks whether to trust an unknown host key (`... Are you sure you want to continue
    /// connecting (yes/no/[fingerprint])?`): the answer is `yes`, `no` or the fingerprint.
    HostKey,
    /// Any other question: shown as it is, the answer is not shown (it may be a secret).
    Other,
}

impl CredentialKind {
    /// Whether the answer is a secret that the field must hide: everything except
    /// [`CredentialKind::Username`] and [`CredentialKind::HostKey`].
    pub fn is_secret(self) -> bool {
        !matches!(self, Self::Username | Self::HostKey)
    }

    /// The kind of a git or ssh prompt (with `LC_ALL=C` the texts are English).
    pub fn of_prompt(prompt: &str) -> Self {
        let lower = prompt.to_ascii_lowercase();
        if lower.contains("(yes/no") || lower.contains("continue connecting") {
            Self::HostKey
        } else if lower.contains("passphrase") {
            Self::Passphrase
        } else if lower.starts_with("username") {
            Self::Username
        } else if lower.contains("password") {
            Self::Password
        } else {
            Self::Other
        }
    }
}

/// A question from git or ssh for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRequest {
    pub kind: CredentialKind,
    /// git's or ssh's text, verbatim (e.g. `Password for 'https://ada@example.com': `): to show to
    /// the user, who can see what is asked and for which host.
    pub prompt: String,
    /// The remote of the command (name, or the URL for `repo.clone`), if known.
    pub remote: Option<String>,
    /// The URL in a git prompt (`Username for '<url>'`), if there is one.
    pub url: Option<String>,
}

impl CredentialRequest {
    /// The request for `prompt`, with its kind and URL recognized.
    pub fn new(prompt: impl Into<String>, remote: Option<String>) -> Self {
        let prompt = prompt.into();
        let kind = CredentialKind::of_prompt(&prompt);
        let url = match kind {
            CredentialKind::Username | CredentialKind::Password => quoted_url(&prompt),
            _ => None,
        };
        Self {
            kind,
            prompt,
            remote,
            url,
        }
    }
}

/// The text between the quotes in `... for '<url>': `.
fn quoted_url(prompt: &str) -> Option<String> {
    let start = prompt.find(" for '")? + " for '".len();
    let len = prompt[start..].find('\'')?;
    let url = &prompt[start..start + len];
    url.contains("://").then(|| url.to_owned())
}

/// Asks the user for credentials on behalf of git: set by the app on the backend (desktop: a
/// dialog), never by the MCP server.
///
/// [`ask`](CredentialPrompter::ask) is **blocking** and called from the worker thread that runs
/// the git command (never the UI thread): it returns when the user answers. `None` = cancelled:
/// git fails and the command ends with
/// [`EngineError::AuthCancelled`](crate::EngineError::AuthCancelled), without retrying. The
/// returned answer is a secret: whoever implements this must not log or keep it.
pub trait CredentialPrompter: Send + Sync {
    fn ask(&self, request: CredentialRequest) -> Option<String>;

    /// Whether the command about to start may ask at all. Called once per network command, on
    /// the thread that runs it, before git starts: `false` = as without a prompter (a missing
    /// credential is [`EngineError::Auth`](crate::EngineError::Auth)). The desktop app says no
    /// for its automatic fetch, which must never open a dialog by itself.
    fn asks_now(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_are_classified() {
        let cases = [
            (
                "Username for 'http://127.0.0.1:8080': ",
                CredentialKind::Username,
            ),
            (
                "Password for 'https://ada@example.com': ",
                CredentialKind::Password,
            ),
            ("ada@example.com's password: ", CredentialKind::Password),
            (
                "Enter passphrase for key '/home/ada/.ssh/id_ed25519': ",
                CredentialKind::Passphrase,
            ),
            (
                "The authenticity of host 'example.com (1.2.3.4)' can't be established.\nED25519 \
                 key fingerprint is SHA256:x.\nAre you sure you want to continue connecting \
                 (yes/no/[fingerprint])? ",
                CredentialKind::HostKey,
            ),
            ("Enter PIN for 'token': ", CredentialKind::Other),
        ];
        for (prompt, kind) in cases {
            assert_eq!(CredentialKind::of_prompt(prompt), kind, "{prompt}");
        }
        assert!(CredentialKind::Password.is_secret());
        assert!(CredentialKind::Other.is_secret());
        assert!(!CredentialKind::Username.is_secret());
    }

    #[test]
    fn url_comes_from_git_prompts_only() {
        let r = CredentialRequest::new(
            "Username for 'https://example.com': ",
            Some("origin".into()),
        );
        assert_eq!(r.url.as_deref(), Some("https://example.com"));
        assert_eq!(r.remote.as_deref(), Some("origin"));
        let r = CredentialRequest::new("Enter passphrase for key '/k': ", None);
        assert_eq!(r.url, None);
        let r = CredentialRequest::new("Password for 'not a url': ", None);
        assert_eq!(r.url, None);
    }
}
