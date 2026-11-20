//! The git identity: `config.identity` and `config.set_identity` (`impl-docs/git/config.md`).
//! The types and the checks are in `git/config.rs`. Both work without an open repo (the global
//! configuration, through the [`RepoOpener`](crate::RepoOpener)); no journal entry, because no
//! ref moves: the values before the write are in the result.

use super::{NoParams, Out, to_json};
use crate::{
    EngineError, IdentityFile, IdentityKey, IdentityScope, Message, MessageArg, PreviousAll,
    Session, SetIdentityParams, SetIdentityResult, identity_decided_elsewhere,
    identity_local_without_repo,
};

/// `user.name` and `user.email` as git reads them for the open repo (or, with no repo open,
/// outside any repo), where they come from, the local and global values and the environment
/// variables that take part.
///
/// Result: [`Identity`](crate::Identity).
pub(super) fn config_identity(s: &mut Session, _: NoParams) -> Out {
    Ok(to_json(s.identity()?))
}

/// Writes the given keys in the local or global configuration file, one `git config` each, name
/// first. Before anything is written the file is read ([`Session::identity_file`]):
///
/// - a key that another file decides (an `include`d file sets it after the file's own value) is
///   refused, unless the value asked for is the one that file already gives: writing would
///   replace the file's own value without changing what git reads;
/// - a value equal to the file's own is not written.
///
/// `previous` is what the file itself held. After the writes the file is read again and checked;
/// every error raised once something was written says what the written keys held before.
///
/// Result: [`SetIdentityResult`].
pub(super) fn config_set_identity(s: &mut Session, params: SetIdentityParams) -> Out {
    let changes = params.check()?;
    let scope = params.scope;
    s.check_writer()?;
    if scope == IdentityScope::Local && s.path().is_none() {
        return Err(identity_local_without_repo());
    }
    let before = s.identity_file(scope)?;
    let previous = before.own();
    // Everything is decided before the first write: a refusal leaves both keys as they were.
    let mut plan: Vec<(IdentityKey, Option<&str>)> = Vec::new();
    for &(key, wanted) in &changes {
        let file = before.get(key);
        match &file.elsewhere {
            Some(other) if Some(other.value.as_str()) == wanted => continue,
            Some(other) => return Err(identity_decided_elsewhere(scope, key, other)),
            None => {}
        }
        if file.values.last().map(String::as_str) != wanted {
            plan.push((key, wanted));
        }
    }

    let mut written: Vec<IdentityKey> = Vec::new();
    for &(key, wanted) in &plan {
        if let Err(error) = s.set_identity(scope, key, wanted) {
            return Err(match written.is_empty() {
                true => error,
                false => after_write(msg!(
                    "engine-config-partial",
                    error = error,
                    written = was(&before, &written)
                )),
            });
        }
        written.push(key);
    }
    if written.is_empty() {
        return Ok(to_json(SetIdentityResult {
            identity: s.identity()?,
            previous,
            previous_all: PreviousAll::default(),
            changed: false,
        }));
    }

    let reread = |error: EngineError| {
        after_write(msg!(
            "engine-config-reread-failed",
            error = error,
            written = was(&before, &written)
        ))
    };
    let after = s.identity_file(scope).map_err(reread)?;
    check_applied(&after, scope, &plan, &before)?;
    let identity = s.identity().map_err(reread)?;
    let mut previous_all = PreviousAll::default();
    for &key in &written {
        previous_all.record(key, &before.get(key).values);
    }
    Ok(to_json(SetIdentityResult {
        identity,
        previous,
        previous_all,
        changed: true,
    }))
}

/// An error raised after something was written: an explanation of the engine around git's text.
fn after_write(reason: Message) -> EngineError {
    EngineError::GitWithReason {
        command: "config".to_owned(),
        reason,
    }
}

/// What the `written` keys held in the file before the write, for the errors raised after it:
/// "`user.name` was “A”, `user.email` was not set" (every value, if a key had several).
fn was(before: &IdentityFile, written: &[IdentityKey]) -> MessageArg {
    let list = written
        .iter()
        .map(|&key| {
            let values = &before.get(key).values;
            let previous = match values.is_empty() {
                true => MessageArg::Joined(vec![msg!("engine-config-unset")], "".into()),
                false => MessageArg::Joined(
                    values
                        .iter()
                        .map(|value| msg!("engine-config-value", value = value))
                        .collect(),
                    ", ".into(),
                ),
            };
            msg!("engine-config-was", key = key.key(), previous = previous)
        })
        .collect();
    MessageArg::Joined(list, ", ".into())
}

/// Error if, after the writes, the file does not hold the requested value of a written key or
/// another file now decides it (written by someone else in between): git wrote the file, the
/// error says what it reads and what the written keys held before.
fn check_applied(
    after: &IdentityFile,
    scope: IdentityScope,
    plan: &[(IdentityKey, Option<&str>)],
    before: &IdentityFile,
) -> Result<(), EngineError> {
    let written: Vec<IdentityKey> = plan.iter().map(|(key, _)| *key).collect();
    for &(key, wanted) in plan {
        let file = after.get(key);
        let read = match &file.elsewhere {
            Some(other) => Some(other.value.as_str()),
            None => file.values.last().map(String::as_str),
        };
        if read == wanted {
            continue;
        }
        return Err(after_write(match read {
            Some(value) => msg!(
                "engine-config-still-set",
                key = key.key(),
                scope = scope.as_str(),
                value = value,
                written = was(before, &written)
            ),
            None => msg!(
                "engine-config-not-read",
                key = key.key(),
                scope = scope.as_str(),
                written = was(before, &written)
            ),
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::test_util::same;
    use crate::{
        ConfigScope, EngineError, Identity, IdentityKey, IdentityPair, MemoryBackend, MemoryOpener,
        PreviousAll, Registry, RepoOpener as _, Session, SetIdentityResult, is_missing_identity,
    };
    use serde_json::{Value, json};
    use std::sync::Arc;

    /// A session with no open repo on an opener with a repo without identity at `repo` and one
    /// with a local identity at `named`.
    fn setup() -> (Registry, Session, Arc<MemoryOpener>) {
        let mut opener = MemoryOpener::new();
        let mut repo = MemoryBackend::sample();
        repo.clear_identity();
        opener.insert("repo", repo);
        let mut named = MemoryBackend::sample();
        named.set_identity("Loc", "loc@example.com");
        opener.insert("named", named);
        let opener = Arc::new(opener);
        (Registry::builtin(), Session::new(opener.clone()), opener)
    }

    fn open(r: &Registry, s: &mut Session, path: &str) {
        r.run("repo.open", s, json!({ "path": path })).unwrap();
    }

    fn get(r: &Registry, s: &mut Session) -> Value {
        r.run("config.identity", s, json!({})).unwrap()
    }

    fn set(r: &Registry, s: &mut Session, params: Value) -> Result<Value, EngineError> {
        r.run("config.set_identity", s, params)
    }

    #[test]
    fn nothing_set_with_and_without_a_repo() {
        let nothing = json!({
            "name": {"value": null, "scope": null},
            "email": {"value": null, "scope": null},
            "local": {"name": null, "email": null},
            "global": {"name": null, "email": null},
            "env": [],
        });
        let (r, mut s, _) = setup();
        assert_eq!(get(&r, &mut s), nothing);
        open(&r, &mut s, "repo");
        assert_eq!(get(&r, &mut s), nothing);
    }

    #[test]
    fn global_is_written_and_read_without_a_repo() {
        let (r, mut s, opener) = setup();
        let out = set(
            &r,
            &mut s,
            json!({"scope": "global", "name": "Glob", "email": "glob@example.com"}),
        )
        .unwrap();
        assert_eq!(out["name"], json!({"value": "Glob", "scope": "global"}));
        assert_eq!(
            out["email"],
            json!({"value": "glob@example.com", "scope": "global"})
        );
        assert_eq!(out["local"], json!({"name": null, "email": null}));
        assert_eq!(
            out["global"],
            json!({"name": "Glob", "email": "glob@example.com"})
        );
        assert_eq!(out["previous"], json!({"name": null, "email": null}));
        assert_eq!(out["changed"], true);
        assert!(s.path().is_none(), "no repo was opened");
        assert_eq!(
            opener.identity().unwrap().global.name.as_deref(),
            Some("Glob")
        );

        // Every repo of the opener sees it; a local value wins over it.
        open(&r, &mut s, "repo");
        assert_eq!(get(&r, &mut s)["name"]["scope"], "global");
        open(&r, &mut s, "named");
        let out = get(&r, &mut s);
        assert_eq!(out["name"], json!({"value": "Loc", "scope": "local"}));
        assert_eq!(out["global"]["name"], "Glob");
        assert_eq!(out["local"]["email"], "loc@example.com");
    }

    #[test]
    fn local_needs_an_open_repo_and_wins_over_global() {
        let (r, mut s, opener) = setup();
        let err = set(&r, &mut s, json!({"scope": "local", "name": "X"})).unwrap_err();
        assert!(
            matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-config-local-no-repo"),
            "{err}"
        );
        assert!(err.to_string().contains("\"scope\": \"global\""), "{err}");

        set(&r, &mut s, json!({"scope": "global", "name": "Glob"})).unwrap();
        open(&r, &mut s, "repo");
        let out = set(
            &r,
            &mut s,
            json!({"scope": "local", "name": "Here", "email": "here@example.com"}),
        )
        .unwrap();
        assert_eq!(out["name"], json!({"value": "Here", "scope": "local"}));
        assert_eq!(out["global"], json!({"name": "Glob", "email": null}));
        assert_eq!(out["previous"], json!({"name": null, "email": null}));
        assert_eq!(out["changed"], true);
        // The global configuration is untouched.
        assert_eq!(
            opener.identity().unwrap().global,
            IdentityPair {
                name: Some("Glob".to_owned()),
                email: None
            }
        );
    }

    #[test]
    fn a_key_not_given_is_not_touched_and_empty_unsets() {
        let (r, mut s, _) = setup();
        open(&r, &mut s, "named");
        let out = set(
            &r,
            &mut s,
            json!({"scope": "local", "email": "new@example.com"}),
        )
        .unwrap();
        assert_eq!(
            out["local"],
            json!({"name": "Loc", "email": "new@example.com"})
        );
        assert_eq!(
            out["previous"],
            json!({"name": "Loc", "email": "loc@example.com"})
        );

        let out = set(&r, &mut s, json!({"scope": "local", "name": ""})).unwrap();
        assert_eq!(
            out["local"],
            json!({"name": null, "email": "new@example.com"})
        );
        assert_eq!(out["name"], json!({"value": null, "scope": null}));
        assert_eq!(out["previous"]["name"], "Loc");
        assert_eq!(out["changed"], true);

        // Unsetting what is not there is not an error and writes nothing.
        let out = set(&r, &mut s, json!({"scope": "local", "name": ""})).unwrap();
        assert_eq!(out["changed"], false);
        assert_eq!(out["previous"]["name"], Value::Null);
        let out = set(
            &r,
            &mut s,
            json!({"scope": "global", "name": "", "email": ""}),
        )
        .unwrap();
        assert_eq!(out["changed"], false);
    }

    #[test]
    fn equal_values_write_nothing() {
        let (r, mut s, opener) = setup();
        open(&r, &mut s, "named");
        // A write would fail: the same values never reach the backend.
        opener.fail_identity_writes(IdentityKey::Name, "error: could not lock config file");
        let out = set(
            &r,
            &mut s,
            json!({"scope": "local", "name": "Loc", "email": "loc@example.com"}),
        )
        .unwrap();
        assert_eq!(out["changed"], false);
        assert_eq!(
            out["previous"],
            json!({"name": "Loc", "email": "loc@example.com"})
        );
        // Only the email differs: only the email is written.
        let out = set(
            &r,
            &mut s,
            json!({"scope": "local", "name": "Loc", "email": "other@example.com"}),
        )
        .unwrap();
        assert_eq!(out["changed"], true);
        assert_eq!(out["local"]["email"], "other@example.com");
    }

    #[test]
    fn a_failed_write_reports_git_and_what_was_already_written() {
        let (r, mut s, opener) = setup();
        open(&r, &mut s, "named");
        opener.fail_identity_writes(IdentityKey::Email, "error: could not lock config file");
        // The first write fails: git's error as it is, nothing written.
        let err = set(
            &r,
            &mut s,
            json!({"scope": "local", "email": "x@example.com"}),
        )
        .unwrap_err();
        assert!(
            matches!(&err, EngineError::Git { command, .. } if command == "config"),
            "{err}"
        );
        assert_eq!(get(&r, &mut s)["local"]["email"], "loc@example.com");

        // The name is written, then the email fails: the error names the old name.
        let err = set(
            &r,
            &mut s,
            json!({"scope": "local", "name": "New", "email": "x@example.com"}),
        )
        .unwrap_err();
        let EngineError::GitWithReason { command, reason } = &err else {
            panic!("{err}");
        };
        assert_eq!(command, "config");
        assert_eq!(reason.key(), "engine-config-partial");
        let text = err.to_string();
        assert!(text.contains("could not lock config file"), "{text}");
        assert!(
            text.contains("`user.name`") && text.contains("“Loc”"),
            "{text}"
        );
        assert_eq!(
            get(&r, &mut s)["local"],
            json!({"name": "New", "email": "loc@example.com"})
        );

        // The same with a name that was not set before.
        let err = set(
            &r,
            &mut s,
            json!({"scope": "global", "name": "Glob", "email": "x@example.com"}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("was not set"), "{err}");
    }

    /// A file included by the global one sets the name after it: the global file is not
    /// written for that key (its own value would be replaced for nothing), unless the value
    /// asked for is the one already in effect.
    #[test]
    fn a_key_decided_by_an_included_file_is_refused_before_writing() {
        let (r, mut s, opener) = setup();
        set(
            &r,
            &mut s,
            json!({"scope": "global", "name": "Own", "email": "own@example.com"}),
        )
        .unwrap();
        opener.set_included_identity(IdentityKey::Name, "/home/ada/work.cfg", "Included");
        let out = get(&r, &mut s);
        assert_eq!(out["name"], json!({"value": "Included", "scope": "global"}));
        assert_eq!(out["global"]["name"], "Included");

        for params in [
            json!({"scope": "global", "name": "Wanted"}),
            json!({"scope": "global", "name": ""}),
            // The email could be written, but nothing is when a key is refused.
            json!({"scope": "global", "email": "new@example.com", "name": "Wanted"}),
        ] {
            let err = set(&r, &mut s, params.clone()).unwrap_err();
            assert!(
                matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-config-decided-elsewhere"),
                "{params}: {err}"
            );
            let text = err.to_string();
            assert!(text.contains("/home/ada/work.cfg"), "{text}");
            assert!(
                text.contains("“Included”") && text.contains("user.name"),
                "{text}"
            );
            assert!(text.contains("nothing was written"), "{text}");
        }
        // The file's own values are as they were.
        let file = opener.global_identity_file().unwrap();
        assert_eq!(file.name.values, ["Own"]);
        assert_eq!(file.email.values, ["own@example.com"]);

        // The value already in effect: nothing to write. The other key is written, and
        // `previous` is what the file itself held, not what the included file gives.
        let out = set(
            &r,
            &mut s,
            json!({"scope": "global", "name": "Included", "email": "new@example.com"}),
        )
        .unwrap();
        assert_eq!(out["changed"], true);
        assert_eq!(
            out["previous"],
            json!({"name": "Own", "email": "own@example.com"})
        );
        assert_eq!(out.get("previous_all"), None);
        assert_eq!(opener.global_identity_file().unwrap().name.values, ["Own"]);
        let out = set(&r, &mut s, json!({"scope": "global", "name": "Included"})).unwrap();
        assert_eq!(out["changed"], false);

        // A repo sees the same, and its local scope is not concerned.
        open(&r, &mut s, "repo");
        let err = set(&r, &mut s, json!({"scope": "global", "name": "Wanted"})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        let out = set(&r, &mut s, json!({"scope": "local", "name": "Here"})).unwrap();
        assert_eq!(out["name"], json!({"value": "Here", "scope": "local"}));
    }

    /// The check after the writes (someone else changed the files in between): the error says
    /// what git reads and every value the written keys held before.
    #[test]
    fn a_write_that_did_not_take_effect_reports_what_was_there_before() {
        use super::check_applied;
        use crate::{IdentityElsewhere, IdentityFile, IdentityFileKey, IdentityScope};
        let key = |values: &[&str], elsewhere: Option<(&str, &str)>| IdentityFileKey {
            values: values.iter().map(|v| (*v).to_owned()).collect(),
            elsewhere: elsewhere.map(|(file, value)| IdentityElsewhere {
                file: file.to_owned(),
                value: value.to_owned(),
            }),
        };
        let before = IdentityFile {
            name: key(&["First", "Second"], None),
            email: key(&[], None),
        };
        let plan = [
            (IdentityKey::Name, Some("New")),
            (IdentityKey::Email, Some("new@example.com")),
        ];
        let applied = IdentityFile {
            name: key(&["New"], None),
            email: key(&["new@example.com"], None),
        };
        check_applied(&applied, IdentityScope::Global, &plan, &before).unwrap();

        let was = "`user.name` was “First”, “Second”, `user.email` was not set";
        for (after, reason, read) in [
            (
                IdentityFile {
                    name: key(&["New"], Some(("/h/inc.cfg", "Other"))),
                    ..applied.clone()
                },
                "engine-config-still-set",
                "“Other”",
            ),
            (
                IdentityFile {
                    name: key(&["Else"], None),
                    ..applied.clone()
                },
                "engine-config-still-set",
                "“Else”",
            ),
            (
                IdentityFile {
                    email: key(&[], None),
                    ..applied
                },
                "engine-config-not-read",
                "user.email",
            ),
        ] {
            let err = check_applied(&after, IdentityScope::Global, &plan, &before).unwrap_err();
            let EngineError::GitWithReason {
                command,
                reason: message,
            } = &err
            else {
                panic!("{err}");
            };
            assert_eq!(command, "config");
            assert_eq!(message.key(), reason);
            let text = err.to_string();
            assert!(text.contains(read) && text.contains("global"), "{text}");
            assert!(text.contains(was), "{text}");
        }
    }

    #[test]
    fn invalid_parameters_are_refused_before_anything_is_read() {
        let (r, mut s, opener) = setup();
        for bad in [
            json!({}),
            json!({"scope": "global"}),
            json!({"name": "A"}),
            json!({"scope": "system", "name": "A"}),
            json!({"scope": "global", "name": "A\nB"}),
            json!({"scope": "global", "email": "a\u{0}b"}),
            json!({"scope": "global", "name": "A <a@b>"}),
            json!({"scope": "global", "name": " .,;"}),
            json!({"scope": "global", "name": "A", "file": "/etc/gitconfig"}),
            json!({"scope": "global", "name": "A", "confirm": true}),
        ] {
            let err = set(&r, &mut s, bad.clone()).unwrap_err();
            assert!(
                matches!(
                    err,
                    EngineError::InvalidParams {
                        id: "config.set_identity",
                        ..
                    }
                ),
                "{bad}: {err}"
            );
        }
        let err = r
            .run("config.identity", &mut s, json!({"scope": "global"}))
            .unwrap_err();
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
        assert_eq!(opener.identity().unwrap(), Identity::default());
    }

    #[test]
    fn the_environment_is_listed() {
        let (r, mut s, opener) = setup();
        opener.set_identity_env("GIT_COMMITTER_NAME", "Env");
        opener.set_identity_env("EMAIL", "env@example.com");
        assert_eq!(
            get(&r, &mut s)["env"],
            json!(["GIT_COMMITTER_NAME", "EMAIL"])
        );
        open(&r, &mut s, "named");
        // `EMAIL` is only a fallback: the repo has an email.
        assert_eq!(get(&r, &mut s)["env"], json!(["GIT_COMMITTER_NAME"]));
    }

    #[test]
    fn a_commit_fails_without_an_identity_and_works_after_setting_it() {
        let (r, mut s, _) = setup();
        open(&r, &mut s, "repo");
        // The sample has a staged file.
        let err = r
            .run("commit.create", &mut s, json!({"message": "x"}))
            .unwrap_err();
        assert!(err.is_missing_identity(), "{err}");
        assert!(is_missing_identity(&err.to_string()), "{err}");

        // Name only: still no email.
        set(&r, &mut s, json!({"scope": "global", "name": "Glob"})).unwrap();
        let err = r
            .run("commit.create", &mut s, json!({"message": "x"}))
            .unwrap_err();
        assert!(err.is_missing_identity(), "{err}");

        set(
            &r,
            &mut s,
            json!({"scope": "local", "email": "here@example.com"}),
        )
        .unwrap();
        let out = r
            .run("commit.create", &mut s, json!({"message": "x"}))
            .unwrap();
        assert_eq!(out["commit"]["author"]["name"], "Glob", "{out}");
        assert_eq!(out["commit"]["author"]["email"], "here@example.com");
        // The identity is not in the journal: undo takes back the commit, not the names.
        r.run("edit.undo", &mut s, json!({})).unwrap();
        let out = get(&r, &mut s);
        assert_eq!(out["name"]["value"], "Glob");
        assert_eq!(out["email"]["scope"], "local");
        let undo = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(undo, EngineError::NotAllowed(_)), "{undo}");
    }

    #[test]
    fn a_reader_reads_the_identity_and_refuses_to_write_it() {
        let (r, mut s, _) = setup();
        open(&r, &mut s, "named");
        set(&r, &mut s, json!({"scope": "global", "name": "Glob"})).unwrap();
        let mut reader = s.reader().unwrap();
        let out = get(&r, &mut reader);
        assert_eq!(out["name"]["value"], "Loc");
        assert_eq!(out["global"]["name"], "Glob");
        for params in [
            json!({"scope": "global", "name": "X"}),
            json!({"scope": "local", "name": "X"}),
            // Even when nothing would be written.
            json!({"scope": "global", "name": "Glob"}),
        ] {
            let err = set(&r, &mut reader, params.clone()).unwrap_err();
            assert!(
                matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reader-session"),
                "{params}: {err}"
            );
        }
        assert_eq!(get(&r, &mut s)["global"]["name"], "Glob");
    }

    #[test]
    fn catalog_flags() {
        let r = Registry::builtin();
        let read = r.get("config.identity").unwrap();
        assert!(!read.writes && read.reader && !read.outside_repo);
        let write = r.get("config.set_identity").unwrap();
        assert!(write.writes && !write.reader && !write.outside_repo);
    }

    /// The typed results read back the JSON of the commands: the UI deserializes into them.
    #[test]
    fn results_round_trip() {
        let (r, mut s, _) = setup();
        open(&r, &mut s, "named");
        let set: SetIdentityResult = same(
            set(
                &r,
                &mut s,
                json!({"scope": "global", "name": "Glob", "email": "glob@example.com"}),
            )
            .unwrap(),
        );
        assert!(set.changed);
        assert_eq!(set.previous, IdentityPair::default());
        assert_eq!(set.previous_all, PreviousAll::default());
        assert_eq!(set.identity.name.scope, Some(ConfigScope::Local));
        let read: Identity = same(get(&r, &mut s));
        assert_eq!(read, set.identity);
        assert_eq!(read.configured(), Some(("Loc", "loc@example.com")));
    }
}
