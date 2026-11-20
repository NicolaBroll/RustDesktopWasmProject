//! The git identity (`user.name`, `user.email`) of `config.identity` and `config.set_identity`
//! (`impl-docs/git/config.md`): the types, the checks and how the values git lists become the
//! result, shared by the backends and the openers.

use crate::EngineError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The environment variables that give git a name or an email, in the order they are listed in
/// [`Identity::env`]. `GIT_AUTHOR_*` and `GIT_COMMITTER_*` win over the configuration; `EMAIL`
/// is used only when no `user.email` is configured.
pub const IDENTITY_ENV: [&str; 5] = [
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "EMAIL",
];

/// Where a configuration value comes from (`git config --show-scope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConfigScope {
    /// The repo's own file (`.git/config`) and the files it includes.
    Local,
    /// The user's file (`~/.gitconfig` or `$XDG_CONFIG_HOME/git/config`) and its includes.
    Global,
    /// The system file (`/etc/gitconfig`) and its includes.
    System,
    /// Anything else: the worktree file (`extensions.worktreeConfig`), the command line.
    Other,
}

impl ConfigScope {
    /// The scope git prints: `local`, `global`, `system`; `worktree`, `command` and any other
    /// name are [`ConfigScope::Other`].
    pub fn from_git(scope: &str) -> Self {
        match scope {
            "local" => Self::Local,
            "global" => Self::Global,
            "system" => Self::System,
            _ => Self::Other,
        }
    }
}

/// The file `config.set_identity` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IdentityScope {
    /// The open repo's file (`git config --local`): only this repository.
    Local,
    /// The user's file (`git config --global`): all the user's repositories.
    Global,
}

impl IdentityScope {
    /// `local` or `global`, as in the parameters.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Global => "global",
        }
    }
}

/// One of the two keys of the identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityKey {
    Name,
    Email,
}

impl IdentityKey {
    /// Name, then email: the order of the writes.
    pub const ALL: [Self; 2] = [Self::Name, Self::Email];

    /// The configuration key: `user.name` or `user.email`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Name => "user.name",
            Self::Email => "user.email",
        }
    }

    /// The key of a `git config` listing (lowercase, as git prints it).
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "user.name" => Some(Self::Name),
            "user.email" => Some(Self::Email),
            _ => None,
        }
    }
}

/// The value git uses for one key and the scope it comes from; both `null` when no file sets it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityValue {
    pub value: Option<String>,
    pub scope: Option<ConfigScope>,
}

/// Name and email of one scope; `null` = not set there.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityPair {
    pub name: Option<String>,
    pub email: Option<String>,
}

impl IdentityPair {
    /// The value of `key`.
    pub fn get(&self, key: IdentityKey) -> Option<&str> {
        match key {
            IdentityKey::Name => self.name.as_deref(),
            IdentityKey::Email => self.email.as_deref(),
        }
    }

    /// Sets (`Some`) or removes (`None`) the value of `key`.
    pub fn set(&mut self, key: IdentityKey, value: Option<&str>) {
        let slot = match key {
            IdentityKey::Name => &mut self.name,
            IdentityKey::Email => &mut self.email,
        };
        *slot = value.map(str::to_owned);
    }
}

/// Result of `config.identity`: the name and email git would use, where they come from, and the
/// values of the two scopes `config.set_identity` writes.
///
/// A value is the last one git reads (a key set twice, or set again by an included file, counts
/// once: the last). A key set to an empty value is `""`, not `null`: it hides the scopes below
/// and git then has no identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// `user.name` as git reads it for the open repo (without a repo: outside any repo).
    pub name: IdentityValue,
    /// `user.email`, likewise.
    pub email: IdentityValue,
    /// The values of the open repo's configuration; both `null` without an open repo.
    pub local: IdentityPair,
    /// The values of the user's global configuration.
    pub global: IdentityPair,
    /// The environment variables among [`IDENTITY_ENV`] that are set for git and take part in
    /// the identity whatever the configuration says: `GIT_AUTHOR_*`/`GIT_COMMITTER_*` when
    /// present (even empty), `EMAIL` when not empty and no `user.email` is configured.
    pub env: Vec<String>,
}

impl Identity {
    /// Builds the result from what git lists: `entries` are the `user.name`/`user.email` values
    /// with their scope **in the order git reads them** (system, global, local, worktree,
    /// command line: the last one wins), `env` gives the value of an environment variable as
    /// git will see it (`None` = not set).
    pub fn from_entries(
        entries: impl IntoIterator<Item = (ConfigScope, IdentityKey, String)>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let mut identity = Self::default();
        for (scope, key, value) in entries {
            match scope {
                ConfigScope::Local => identity.local.set(key, Some(&value)),
                ConfigScope::Global => identity.global.set(key, Some(&value)),
                ConfigScope::System | ConfigScope::Other => {}
            }
            let effective = match key {
                IdentityKey::Name => &mut identity.name,
                IdentityKey::Email => &mut identity.email,
            };
            *effective = IdentityValue {
                value: Some(value),
                scope: Some(scope),
            };
        }
        let configured_email = identity
            .email
            .value
            .as_deref()
            .is_some_and(|e| !e.is_empty());
        identity.env = IDENTITY_ENV
            .iter()
            .filter(|name| match (**name, env(name)) {
                (_, None) => false,
                // git ignores an empty `EMAIL`, and reads it only without `user.email`.
                ("EMAIL", Some(value)) => !value.is_empty() && !configured_email,
                (_, Some(_)) => true,
            })
            .map(|name| (*name).to_owned())
            .collect();
        identity
    }

    /// The values of `scope`.
    pub fn of(&self, scope: IdentityScope) -> &IdentityPair {
        match scope {
            IdentityScope::Local => &self.local,
            IdentityScope::Global => &self.global,
        }
    }

    /// Name and email from the configuration when both are set and not empty: what a commit
    /// needs (the environment is not looked at).
    pub fn configured(&self) -> Option<(&str, &str)> {
        fn set(value: &IdentityValue) -> Option<&str> {
            value.value.as_deref().filter(|v| !v.is_empty())
        }
        Some((set(&self.name)?, set(&self.email)?))
    }
}

/// Parameters of `config.set_identity`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetIdentityParams {
    /// `local` (the open repository only) or `global` (all the user's repositories; works
    /// without an open repo).
    pub scope: IdentityScope,
    /// New `user.name`; an empty string removes the key from that scope; absent = not touched.
    /// No NUL, line break, `<` or `>`; not made only of the characters git strips (spaces and
    /// `.,:;"'\`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New `user.email`, with the same rules as `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl SetIdentityParams {
    /// The keys to write, name first, each with the wanted value (`None` = remove the key).
    /// [`EngineError::InvalidParams`] if neither key is given or a value cannot be an identity.
    pub fn check(&self) -> Result<Vec<(IdentityKey, Option<&str>)>, EngineError> {
        const ID: &str = "config.set_identity";
        let invalid = |reason| EngineError::InvalidParams { id: ID, reason };
        let mut changes = Vec::new();
        for key in IdentityKey::ALL {
            let given = match key {
                IdentityKey::Name => &self.name,
                IdentityKey::Email => &self.email,
            };
            let Some(value) = given else { continue };
            if value.contains(['\0', '\n', '\r']) {
                return Err(invalid(msg!("engine-config-value-line", key = key.key())));
            }
            if value.contains(['<', '>']) {
                return Err(invalid(msg!("engine-config-value-angle", key = key.key())));
            }
            if !value.is_empty() && value.chars().all(is_crud) {
                return Err(invalid(msg!("engine-config-value-crud", key = key.key())));
            }
            changes.push((key, Some(value.as_str()).filter(|v| !v.is_empty())));
        }
        if changes.is_empty() {
            return Err(invalid(msg!("engine-config-nothing-given")));
        }
        Ok(changes)
    }
}

/// The characters git strips from the two ends of a name or an email (`crud` in git's
/// `ident.c`): a value made only of them is an empty identity.
fn is_crud(c: char) -> bool {
    c <= ' ' || matches!(c, '.' | ',' | ':' | ';' | '<' | '>' | '"' | '\\' | '\'')
}

/// Result of `config.set_identity`: the identity re-read after the write, as in
/// `config.identity`, and what the written file held before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetIdentityResult {
    #[serde(flatten)]
    pub identity: Identity,
    /// The values **of the written file itself** before the write (`null` = the file did not
    /// set the key): the last one when the file had several. Files it includes do not count.
    pub previous: IdentityPair,
    /// Present only when a key that was written had several values in the file: all of them,
    /// in file order (the write leaves one).
    #[serde(default, skip_serializing_if = "PreviousAll::is_empty")]
    pub previous_all: PreviousAll,
    /// `false` if nothing had to be written: every given value was already the one of the file
    /// (or the one an included file gives).
    pub changed: bool,
}

/// [`SetIdentityResult::previous_all`]: for each written key that had several values, all of
/// them; a key with at most one value is absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviousAll {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub name: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub email: Vec<String>,
}

impl PreviousAll {
    /// No key had several values.
    pub fn is_empty(&self) -> bool {
        self.name.is_empty() && self.email.is_empty()
    }

    /// Records `values` for `key` if they are more than one.
    pub fn record(&mut self, key: IdentityKey, values: &[String]) {
        if values.len() > 1 {
            match key {
                IdentityKey::Name => self.name = values.to_vec(),
                IdentityKey::Email => self.email = values.to_vec(),
            }
        }
    }
}

/// The file `config.set_identity` writes for a scope, as it is now: what the file itself holds
/// and, per key, whether another file decides what git reads in that scope
/// ([`GitConfig::identity_file`](crate::GitConfig::identity_file)).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityFile {
    pub name: IdentityFileKey,
    pub email: IdentityFileKey,
}

/// One key of an [`IdentityFile`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityFileKey {
    /// Every value the written file itself has for the key, in file order (`include`s not
    /// followed); git uses the last.
    pub values: Vec<String>,
    /// Set when the value git reads in this scope is not the file's own last one: a file it
    /// includes (`include`, `includeIf`), or another file of the same scope, sets the key after
    /// it. A write to the file would not change what git reads.
    pub elsewhere: Option<IdentityElsewhere>,
}

/// The other file that decides a key of a scope, and the value it gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityElsewhere {
    /// The file, as git names it (`git config --show-origin`, without `file:`).
    pub file: String,
    pub value: String,
}

impl IdentityFile {
    /// The key `key`.
    pub fn get(&self, key: IdentityKey) -> &IdentityFileKey {
        match key {
            IdentityKey::Name => &self.name,
            IdentityKey::Email => &self.email,
        }
    }

    /// The file's own value of each key (the last one): the `previous` of a write.
    pub fn own(&self) -> IdentityPair {
        IdentityPair {
            name: self.name.values.last().cloned(),
            email: self.email.values.last().cloned(),
        }
    }
}

/// The refusal of an opener that must not write the user's global configuration (the MCP
/// server, confined to its root): nothing was written.
pub fn identity_global_confined() -> EngineError {
    EngineError::NotAllowed(msg!("engine-config-global-confined"))
}

/// `config.set_identity` on a key that another file decides ([`IdentityFileKey::elsewhere`]):
/// refused before anything is written, because the write would replace the file's own value
/// without changing what git reads.
pub fn identity_decided_elsewhere(
    scope: IdentityScope,
    key: IdentityKey,
    elsewhere: &IdentityElsewhere,
) -> EngineError {
    EngineError::NotAllowed(msg!(
        "engine-config-decided-elsewhere",
        key = key.key(),
        scope = scope.as_str(),
        value = elsewhere.value,
        file = elsewhere.file
    ))
}

/// Whether `text` (git's stderr, or the text of an error that contains it) says that a commit
/// could not be created because git has no usable name or email: the cases in which
/// `config.set_identity` is the fix. git's messages (`ident.c`, with `LC_ALL=C`): the "Please
/// tell me who you are" hint, "empty ident name", "unable to auto-detect email address/name",
/// "no email/name was given and auto-detection is disabled" (`user.useConfigOnly`), "name
/// consists only of disallowed characters".
pub fn is_missing_identity(text: &str) -> bool {
    const MARKERS: [&str; 7] = [
        "Please tell me who you are",
        "empty ident name",
        "unable to auto-detect email address",
        "unable to auto-detect name",
        "no email was given and auto-detection is disabled",
        "no name was given and auto-detection is disabled",
        "name consists only of disallowed characters",
    ];
    MARKERS.iter().any(|marker| text.contains(marker))
}

/// `config.set_identity` with `"scope": "local"` and no open repo: nothing was written.
pub fn identity_local_without_repo() -> EngineError {
    EngineError::NotAllowed(msg!("engine-config-local-no-repo"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ConfigScope::{Global, Local, Other, System};
    use IdentityKey::{Email, Name};

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn entry(
        scope: ConfigScope,
        key: IdentityKey,
        value: &str,
    ) -> (ConfigScope, IdentityKey, String) {
        (scope, key, value.to_owned())
    }

    #[test]
    fn the_last_entry_wins_and_each_scope_keeps_its_own() {
        let identity = Identity::from_entries(
            [
                entry(System, Name, "Sys"),
                entry(System, Email, "sys@x"),
                entry(Global, Name, "Glob"),
                entry(Global, Email, "glob@x"),
                entry(Local, Name, "First"),
                entry(Local, Name, "Loc"),
            ],
            no_env,
        );
        assert_eq!(identity.name.value.as_deref(), Some("Loc"));
        assert_eq!(identity.name.scope, Some(Local));
        assert_eq!(identity.email.value.as_deref(), Some("glob@x"));
        assert_eq!(identity.email.scope, Some(Global));
        assert_eq!(identity.local.name.as_deref(), Some("Loc"));
        assert_eq!(identity.local.email, None);
        assert_eq!(identity.global.name.as_deref(), Some("Glob"));
        assert_eq!(identity.global.email.as_deref(), Some("glob@x"));
        assert_eq!(identity.configured(), Some(("Loc", "glob@x")));
        assert_eq!(identity.of(IdentityScope::Global), &identity.global);
        assert!(identity.env.is_empty());
    }

    #[test]
    fn nothing_set_and_other_scopes() {
        let empty = Identity::from_entries([], no_env);
        assert_eq!(empty, Identity::default());
        assert_eq!(empty.configured(), None);

        // The worktree file or the command line: effective, in neither scope.
        let other = Identity::from_entries(
            [entry(Global, Name, "Glob"), entry(Other, Name, "Wt")],
            no_env,
        );
        assert_eq!(other.name.value.as_deref(), Some("Wt"));
        assert_eq!(other.name.scope, Some(Other));
        assert_eq!(other.global.name.as_deref(), Some("Glob"));
        assert_eq!(other.local, IdentityPair::default());
        assert_eq!(other.configured(), None, "no email");

        // An empty local value hides the global one: git has no name.
        let hidden = Identity::from_entries(
            [
                entry(Global, Name, "Glob"),
                entry(Global, Email, "g@x"),
                entry(Local, Name, ""),
            ],
            no_env,
        );
        assert_eq!(hidden.name.value.as_deref(), Some(""));
        assert_eq!(hidden.local.name.as_deref(), Some(""));
        assert_eq!(hidden.configured(), None);
    }

    #[test]
    fn the_environment_variables_that_count_are_listed_in_order() {
        let env = |set: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                set.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let all = &[
            ("EMAIL", "e@x"),
            ("GIT_COMMITTER_EMAIL", "c@x"),
            ("GIT_AUTHOR_NAME", ""),
            ("OTHER", "x"),
        ];
        let identity = Identity::from_entries([], env(all));
        assert_eq!(
            identity.env,
            ["GIT_AUTHOR_NAME", "GIT_COMMITTER_EMAIL", "EMAIL"]
        );
        // `EMAIL` is only a fallback: not listed with a configured email, or when empty.
        let identity = Identity::from_entries([entry(Global, Email, "g@x")], env(all));
        assert_eq!(identity.env, ["GIT_AUTHOR_NAME", "GIT_COMMITTER_EMAIL"]);
        let identity = Identity::from_entries([entry(Global, Email, "")], env(all));
        assert_eq!(
            identity.env,
            ["GIT_AUTHOR_NAME", "GIT_COMMITTER_EMAIL", "EMAIL"]
        );
        let identity = Identity::from_entries([], env(&[("EMAIL", "")]));
        assert!(identity.env.is_empty());
    }

    #[test]
    fn scopes_from_git() {
        for (text, scope) in [
            ("local", Local),
            ("global", Global),
            ("system", System),
            ("worktree", Other),
            ("command", Other),
            ("unknown", Other),
        ] {
            assert_eq!(ConfigScope::from_git(text), scope, "{text}");
        }
        assert_eq!(IdentityKey::from_key("user.name"), Some(Name));
        assert_eq!(IdentityKey::from_key("user.email"), Some(Email));
        assert_eq!(IdentityKey::from_key("user.signingkey"), None);
    }

    fn params(name: Option<&str>, email: Option<&str>) -> SetIdentityParams {
        SetIdentityParams {
            scope: IdentityScope::Global,
            name: name.map(str::to_owned),
            email: email.map(str::to_owned),
        }
    }

    #[test]
    fn checks_the_values() {
        assert_eq!(
            params(Some("Ada L."), Some("ada@example.com"))
                .check()
                .unwrap(),
            [(Name, Some("Ada L.")), (Email, Some("ada@example.com"))]
        );
        // Empty = remove the key; absent = not touched. A leading `-` is a value.
        assert_eq!(params(Some(""), None).check().unwrap(), [(Name, None)]);
        assert_eq!(
            params(None, Some("-x@y")).check().unwrap(),
            [(Email, Some("-x@y"))]
        );
        for (name, email, key) in [
            (None, None, "engine-config-nothing-given"),
            (Some("a\nb"), None, "engine-config-value-line"),
            (Some("a\rb"), None, "engine-config-value-line"),
            (None, Some("a\0b"), "engine-config-value-line"),
            (Some("Ada <ada@x>"), None, "engine-config-value-angle"),
            (None, Some("<ada@x>"), "engine-config-value-angle"),
            (Some(" . "), None, "engine-config-value-crud"),
            (Some("\t"), None, "engine-config-value-crud"),
            (None, Some("'\"\\;:,."), "engine-config-value-crud"),
        ] {
            let err = params(name, email).check().unwrap_err();
            assert!(
                matches!(&err, EngineError::InvalidParams { id: "config.set_identity", reason } if reason.key() == key),
                "{name:?} {email:?}: {err}"
            );
        }
    }

    #[test]
    fn params_refuse_unknown_fields_and_scopes() {
        let ok: SetIdentityParams =
            serde_json::from_value(serde_json::json!({"scope": "local", "name": "A"})).unwrap();
        assert_eq!(ok.scope, IdentityScope::Local);
        assert_eq!(ok.email, None);
        for bad in [
            serde_json::json!({"name": "A"}),
            serde_json::json!({"scope": "system", "name": "A"}),
            serde_json::json!({"scope": "worktree", "name": "A"}),
            serde_json::json!({"scope": "global", "name": "A", "file": "/etc/gitconfig"}),
            serde_json::json!({"scope": "global", "name": 1}),
        ] {
            assert!(
                serde_json::from_value::<SetIdentityParams>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    /// git's real messages (2.30 and 2.56, `LC_ALL=C`).
    #[test]
    fn recognises_a_missing_identity_in_git_messages() {
        for text in [
            "Author identity unknown\n\n*** Please tell me who you are.\n\nRun\n\n  git config \
             --global user.email \"you@example.com\"\n  git config --global user.name \"Your \
             Name\"\n\nto set your account's default identity.\nOmit --global to set the \
             identity only in this repository.\n\nfatal: unable to auto-detect email address \
             (got 'ada@host.(none)')",
            "Committer identity unknown\n\n*** Please tell me who you are.\n\nfatal: empty \
             ident name (for <ada@host>) not allowed",
            "fatal: empty ident name (for <ada@example.com>) not allowed",
            "fatal: unable to auto-detect email address (got 'ada@host.(none)')",
            "fatal: unable to auto-detect name (got 'ada')",
            "fatal: no email was given and auto-detection is disabled",
            "fatal: no name was given and auto-detection is disabled",
            "fatal: name consists only of disallowed characters: ...",
            "git commit: fatal: empty ident name (for <a@b>) not allowed",
        ] {
            assert!(is_missing_identity(text), "{text}");
        }
        for text in [
            "",
            "error: gpg failed to sign the data",
            "fatal: unable to auto-detect the remote",
            "hint: Waiting for your editor to close the file...",
            "error: pathspec 'who you are' did not match any file(s) known to git",
        ] {
            assert!(!is_missing_identity(text), "{text}");
        }
    }
}
