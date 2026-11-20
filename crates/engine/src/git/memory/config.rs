//! The git identity in memory (`config.identity`, `config.set_identity`,
//! `impl-docs/git/config.md`): each repo has its local values, a [`MemoryOpener`] and the repos
//! it opens share the user's global values and environment, like the repos of one user.

use super::{MemoryBackend, MemoryOpener};
use crate::{
    ConfigScope, EngineError, GitConfig, Identity, IdentityElsewhere, IdentityFile,
    IdentityFileKey, IdentityKey, IdentityPair, IdentityScope,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// The user's side of the configuration: the global identity and the environment variables.
/// Clones share it (a repo opened by a [`MemoryOpener`] sees what the opener writes);
/// [`UserConfig::detached`] gives an independent copy.
#[derive(Debug, Clone, Default)]
pub(super) struct UserConfig(Arc<Mutex<UserState>>);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UserState {
    identity: IdentityPair,
    env: BTreeMap<String, String>,
    /// Writes of this key fail with this text, in every scope.
    failure: Option<(IdentityKey, String)>,
    /// Keys of the global scope that a file included by the global one sets, after the global
    /// file's own value: key, file, value ([`MemoryOpener::set_included_identity`]).
    included: Vec<(IdentityKey, IdentityElsewhere)>,
}

impl UserConfig {
    fn lock(&self) -> MutexGuard<'_, UserState> {
        // Every write is one assignment: a panic while holding the lock leaves it consistent.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn state(&self) -> UserState {
        self.lock().clone()
    }

    /// A copy that no longer follows this one.
    pub(super) fn detached(&self) -> Self {
        Self(Arc::new(Mutex::new(self.state())))
    }

    /// The error of a write of `key`, if writes of that key are set to fail.
    fn check_write(&self, key: IdentityKey) -> Result<(), EngineError> {
        match &self.lock().failure {
            Some((failing, message)) if *failing == key => Err(EngineError::Git {
                command: "config".to_owned(),
                stderr: message.clone(),
            }),
            _ => Ok(()),
        }
    }
}

/// Two repos are equal when their user configurations hold the same values.
impl PartialEq for UserConfig {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.state() == other.state()
    }
}

impl Eq for UserConfig {}

/// The identity git would report: the global values (the file's own, then the ones of the
/// files it includes), then `local` (if a repo is open), as git reads them.
fn report(user: &UserConfig, local: Option<&IdentityPair>) -> Identity {
    let user = user.state();
    fn pair(
        scope: ConfigScope,
        pair: Option<&IdentityPair>,
    ) -> impl Iterator<Item = (ConfigScope, IdentityKey, String)> + '_ {
        IdentityKey::ALL.into_iter().filter_map(move |key| {
            let value = pair?.get(key)?;
            Some((scope, key, value.to_owned()))
        })
    }
    let included = user
        .included
        .iter()
        .map(|(key, other)| (ConfigScope::Global, *key, other.value.clone()));
    let entries = pair(ConfigScope::Global, Some(&user.identity))
        .chain(included)
        .chain(pair(ConfigScope::Local, local));
    Identity::from_entries(entries, |name| user.env.get(name).cloned())
}

/// The [`IdentityFile`] of a file that holds `own`, with `included` deciding some keys.
fn file(own: &IdentityPair, included: &[(IdentityKey, IdentityElsewhere)]) -> IdentityFile {
    let key = |key: IdentityKey| IdentityFileKey {
        values: own.get(key).map(str::to_owned).into_iter().collect(),
        elsewhere: included
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, other)| other.clone()),
    };
    IdentityFile {
        name: key(IdentityKey::Name),
        email: key(IdentityKey::Email),
    }
}

impl MemoryBackend {
    /// `user.name` and `user.email` of the repo's own configuration, like `git config --local`.
    /// Without an identity here or in the global configuration (the default of
    /// [`MemoryBackend::new`]) a commit fails, like git without an identity.
    pub fn set_identity(&mut self, name: &str, email: &str) {
        self.local_identity = IdentityPair {
            name: Some(name.to_owned()),
            email: Some(email.to_owned()),
        };
    }

    /// Removes the repo's own `user.name` and `user.email`: the global ones, if any, apply.
    pub fn clear_identity(&mut self) {
        self.local_identity = IdentityPair::default();
    }

    /// Name and email of the committer from the configuration (local over global), if both are
    /// set and not empty.
    pub(super) fn committer(&self) -> Option<(String, String)> {
        let identity = report(&self.user, Some(&self.local_identity));
        let (name, email) = identity.configured()?;
        Some((name.to_owned(), email.to_owned()))
    }

    /// The error of a commit without an identity, with the text of git's hint so that
    /// [`is_missing_identity`](crate::is_missing_identity) recognises it. `who` is `Author` or
    /// `Committer`.
    pub(super) fn identity_unknown(command: &str, who: &str) -> EngineError {
        EngineError::Git {
            command: command.to_owned(),
            stderr: format!(
                "{who} identity unknown\n\n*** Please tell me who you are.\n\nfatal: no \
                 identity in the in-memory repo (`MemoryBackend::set_identity`)"
            ),
        }
    }

    /// The same repo with `user` as its user configuration.
    pub(super) fn with_user(mut self, user: &UserConfig) -> Self {
        self.user = user.clone();
        self
    }
}

impl GitConfig for MemoryBackend {
    fn identity(&self) -> Result<Identity, EngineError> {
        Ok(report(&self.user, Some(&self.local_identity)))
    }

    fn identity_file(&self, scope: IdentityScope) -> Result<IdentityFile, EngineError> {
        Ok(match scope {
            IdentityScope::Local => file(&self.local_identity, &[]),
            IdentityScope::Global => {
                let user = self.user.state();
                file(&user.identity, &user.included)
            }
        })
    }

    fn set_local_identity(
        &mut self,
        key: IdentityKey,
        value: Option<&str>,
    ) -> Result<(), EngineError> {
        self.user.check_write(key)?;
        self.local_identity.set(key, value);
        Ok(())
    }
}

impl MemoryOpener {
    /// Sets the environment variable `name` (e.g. `GIT_AUTHOR_NAME`, `EMAIL`) for the "git" of
    /// this opener and of the repos it opens: `config.identity` lists it in `env`.
    pub fn set_identity_env(&self, name: &str, value: &str) {
        self.user
            .lock()
            .env
            .insert(name.to_owned(), value.to_owned());
    }

    /// From now on a write of `key` (local or global) fails with `stderr`, like a configuration
    /// file that cannot be locked.
    pub fn fail_identity_writes(&self, key: IdentityKey, stderr: &str) {
        self.user.lock().failure = Some((key, stderr.to_owned()));
    }

    /// From now on a file included by the global configuration (`file`, as git would name it)
    /// sets `key` to `value` after the global file's own value: git reads `value` in the global
    /// scope whatever the global file says, and `config.set_identity` refuses to write the key.
    pub fn set_included_identity(&self, key: IdentityKey, file: &str, value: &str) {
        let other = IdentityElsewhere {
            file: file.to_owned(),
            value: value.to_owned(),
        };
        self.user.lock().included.push((key, other));
    }

    /// [`RepoOpener::global_identity_file`](crate::RepoOpener::global_identity_file).
    pub(super) fn user_identity_file(&self) -> IdentityFile {
        let user = self.user.state();
        file(&user.identity, &user.included)
    }

    /// [`RepoOpener::identity`](crate::RepoOpener::identity).
    pub(super) fn user_identity(&self) -> Identity {
        report(&self.user, None)
    }

    /// [`RepoOpener::set_global_identity`](crate::RepoOpener::set_global_identity).
    pub(super) fn set_user_identity(
        &self,
        key: IdentityKey,
        value: Option<&str>,
    ) -> Result<(), EngineError> {
        self.user.check_write(key)?;
        self.user.lock().identity.set(key, value);
        Ok(())
    }
}
