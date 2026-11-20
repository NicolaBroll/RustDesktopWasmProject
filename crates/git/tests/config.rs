//! Integration tests of `config.identity` and `config.set_identity` (`impl-docs/git/config.md`)
//! on real repos and a real global configuration file.
//!
//! The global file is **never the real one**: every git of these tests runs as a [`User`] whose
//! `HOME` and `XDG_CONFIG_HOME` are in a temporary folder (they work with git 2.30, which has no
//! `GIT_CONFIG_GLOBAL`; an inherited `GIT_CONFIG_GLOBAL` is removed), without the system file,
//! and the environment is injected through the opener, not set on the test process.
//! [`User::drop`] checks that the real files did not change.

mod common;

use common::Repo;
use project_engine::{EngineError, IDENTITY_ENV, Registry, RepoOpener as _, Session};
use project_git::CliOpener;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;

/// The global configuration files of whoever runs the tests: must never change.
fn real_global_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for var in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(var) {
            files.push(Path::new(&home).join(".gitconfig"));
            files.push(Path::new(&home).join(".config/git/config"));
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        files.push(Path::new(&xdg).join("git/config"));
    }
    if let Some(file) = std::env::var_os("GIT_CONFIG_GLOBAL") {
        files.push(file.into());
    }
    files
}

/// A user with an empty home in a temporary folder.
struct User {
    home: TempDir,
    real: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl User {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        for real in real_global_files() {
            assert!(
                !real.starts_with(home.path()),
                "the test home must not be the real one"
            );
        }
        let real = real_global_files()
            .into_iter()
            .map(|file| {
                let bytes = std::fs::read(&file).ok();
                (file, bytes)
            })
            .collect();
        Self { home, real }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    /// `~/.gitconfig` of this user.
    fn gitconfig(&self) -> PathBuf {
        self.home().join(".gitconfig")
    }

    fn xdg(&self) -> PathBuf {
        self.home().join("xdg")
    }

    /// The environment that makes git read and write this user's files only.
    fn env(&self) -> [(&'static str, PathBuf); 3] {
        [
            ("HOME", self.home().to_owned()),
            ("XDG_CONFIG_HOME", self.xdg()),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ]
    }

    /// The opener under test, as this user: no global file but the one in the home, and none
    /// of the identity variables inherited from the environment of the tests.
    fn opener(&self) -> CliOpener {
        let mut opener = CliOpener::new().env_remove("GIT_CONFIG_GLOBAL");
        for name in IDENTITY_ENV {
            opener = opener.env_remove(name);
        }
        self.env()
            .into_iter()
            .fold(opener, |opener, (key, value)| opener.env(key, value))
    }

    /// A session of this user with no open repo.
    fn session(&self) -> (Registry, Session) {
        self.session_with(self.opener())
    }

    fn session_with(&self, opener: CliOpener) -> (Registry, Session) {
        (Registry::builtin(), Session::new(Arc::new(opener)))
    }

    /// A session of this user with `repo` open.
    fn open(&self, repo: &Repo) -> (Registry, Session) {
        let (r, mut s) = self.session();
        r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
            .unwrap();
        (r, s)
    }

    /// git run by the test as this user, in `dir`: to prepare and to check independently.
    fn git_in(&self, dir: &Path, args: &[&str]) -> (bool, String) {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(dir).args(args).env("LC_ALL", "C");
        cmd.env_remove("GIT_CONFIG_GLOBAL");
        for name in IDENTITY_ENV {
            cmd.env_remove(name);
        }
        for (key, value) in self.env() {
            cmd.env(key, value);
        }
        let out = cmd.output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim_end().to_owned(),
        )
    }

    /// `git config --global <args>` as this user (outside any repo: in the home).
    fn global(&self, args: &[&str]) -> String {
        let mut all = vec!["config", "--global"];
        all.extend_from_slice(args);
        let (ok, out) = self.git_in(self.home(), &all);
        assert!(ok, "git config --global {args:?}");
        out
    }

    /// The bytes of `~/.gitconfig`, `None` if it does not exist.
    fn gitconfig_bytes(&self) -> Option<Vec<u8>> {
        std::fs::read(self.gitconfig()).ok()
    }
}

impl Drop for User {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        for (file, before) in &self.real {
            assert_eq!(
                &std::fs::read(file).ok(),
                before,
                "{} changed during the test",
                file.display()
            );
        }
    }
}

fn get(r: &Registry, s: &mut Session) -> Value {
    r.run("config.identity", s, json!({})).unwrap()
}

fn set(r: &Registry, s: &mut Session, params: Value) -> Result<Value, EngineError> {
    r.run("config.set_identity", s, params)
}

/// What `git config --local --get-all <key>` prints in the repo (independent of the backend).
fn local_values(user: &User, repo: &Repo, key: &str) -> String {
    user.git_in(repo.path(), &["config", "--local", "--get-all", key])
        .1
}

fn value(v: &str, scope: &str) -> Value {
    json!({ "value": v, "scope": scope })
}

const UNSET: &str = r#"{"scope":null,"value":null}"#;

#[test]
fn nothing_set() {
    let user = User::new();
    let nothing = json!({
        "name": {"value": null, "scope": null},
        "email": {"value": null, "scope": null},
        "local": {"name": null, "email": null},
        "global": {"name": null, "email": null},
        "env": [],
    });
    let (r, mut s) = user.session();
    assert_eq!(get(&r, &mut s), nothing);
    let repo = Repo::new();
    let (r, mut s) = user.open(&repo);
    assert_eq!(get(&r, &mut s), nothing);
    // A read creates no file.
    assert_eq!(user.gitconfig_bytes(), None);
}

#[test]
fn global_only_then_local_over_global() {
    let user = User::new();
    user.global(&["user.name", "Glob Al"]);
    user.global(&["user.email", "glob@example.com"]);
    let repo = Repo::new();
    let (r, mut s) = user.open(&repo);
    let out = get(&r, &mut s);
    assert_eq!(out["name"], value("Glob Al", "global"));
    assert_eq!(out["email"], value("glob@example.com", "global"));
    assert_eq!(out["local"], json!({"name": null, "email": null}));
    assert_eq!(
        out["global"],
        json!({"name": "Glob Al", "email": "glob@example.com"})
    );

    // The same without a repo.
    let (r2, mut s2) = user.session();
    let outside = get(&r2, &mut s2);
    assert_eq!(outside, out);

    // A local name hides the global one; the email still comes from the global file.
    repo.git(&["config", "user.name", "Loc"]);
    let out = get(&r, &mut s);
    assert_eq!(out["name"], value("Loc", "local"));
    assert_eq!(out["email"], value("glob@example.com", "global"));
    assert_eq!(out["local"], json!({"name": "Loc", "email": null}));
    assert_eq!(out["global"]["name"], "Glob Al");
    // An empty local value hides it too, and git then has no email.
    repo.git(&["config", "user.email", ""]);
    let out = get(&r, &mut s);
    assert_eq!(out["email"], value("", "local"));
    assert_eq!(out["local"]["email"], "");
    // Outside the repo nothing changed.
    assert_eq!(get(&r2, &mut s2), outside);
}

#[test]
fn set_local_writes_only_the_repo_file() {
    let user = User::new();
    user.global(&["user.name", "Glob"]);
    let global = user.gitconfig_bytes();
    let repo = Repo::new();
    let (r, mut s) = user.open(&repo);
    let out = set(
        &r,
        &mut s,
        json!({"scope": "local", "name": "Loc", "email": "loc@example.com"}),
    )
    .unwrap();
    assert_eq!(out["name"], value("Loc", "local"));
    assert_eq!(out["email"], value("loc@example.com", "local"));
    assert_eq!(
        out["local"],
        json!({"name": "Loc", "email": "loc@example.com"})
    );
    assert_eq!(out["global"], json!({"name": "Glob", "email": null}));
    assert_eq!(out["previous"], json!({"name": null, "email": null}));
    assert_eq!(out["changed"], true);
    assert_eq!(local_values(&user, &repo, "user.name"), "Loc");
    assert_eq!(local_values(&user, &repo, "user.email"), "loc@example.com");
    assert_eq!(
        user.gitconfig_bytes(),
        global,
        "the global file is untouched"
    );
    // Not in the journal: nothing to undo.
    let undo = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(undo, EngineError::NotAllowed(_)), "{undo}");

    // Only the given key is written; `previous` has both values of the scope.
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
}

#[test]
fn set_global_without_a_repo_writes_the_home_of_the_user() {
    let user = User::new();
    let (r, mut s) = user.session();
    // Local needs a repo: refused, nothing created.
    let err = set(&r, &mut s, json!({"scope": "local", "name": "X"})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-config-local-no-repo"),
        "{err}"
    );
    assert_eq!(user.gitconfig_bytes(), None);

    let out = set(
        &r,
        &mut s,
        json!({"scope": "global", "name": "Glob Al", "email": "glob@example.com"}),
    )
    .unwrap();
    assert_eq!(out["name"], value("Glob Al", "global"));
    assert_eq!(out["email"], value("glob@example.com", "global"));
    assert_eq!(out["local"], json!({"name": null, "email": null}));
    assert_eq!(out["previous"], json!({"name": null, "email": null}));
    assert_eq!(out["changed"], true);
    assert!(s.path().is_none());
    // The file is the one in the temporary home, and git reads it back.
    assert!(user.gitconfig().is_file());
    assert_eq!(user.global(&["--get-all", "user.name"]), "Glob Al");
    assert_eq!(
        user.global(&["--get-all", "user.email"]),
        "glob@example.com"
    );

    // With a repo open the global write is the same, and the repo file is untouched.
    let repo = Repo::new();
    let config = std::fs::read(repo.path().join(".git/config")).unwrap();
    let (r, mut s) = user.open(&repo);
    let out = set(&r, &mut s, json!({"scope": "global", "name": "Other"})).unwrap();
    assert_eq!(out["name"], value("Other", "global"));
    assert_eq!(out["previous"]["name"], "Glob Al");
    assert_eq!(
        std::fs::read(repo.path().join(".git/config")).unwrap(),
        config
    );
    assert_eq!(user.global(&["--get-all", "user.name"]), "Other");
}

#[test]
fn set_global_writes_the_xdg_file_when_that_is_the_one_git_uses() {
    let user = User::new();
    // git's rule: without `~/.gitconfig`, an existing `$XDG_CONFIG_HOME/git/config` is the file.
    let xdg = user.xdg().join("git/config");
    std::fs::create_dir_all(xdg.parent().unwrap()).unwrap();
    std::fs::write(&xdg, "[user]\n\temail = xdg@example.com\n").unwrap();
    let (r, mut s) = user.session();
    let out = set(&r, &mut s, json!({"scope": "global", "name": "Xdg"})).unwrap();
    assert_eq!(
        out["global"],
        json!({"name": "Xdg", "email": "xdg@example.com"})
    );
    assert_eq!(user.gitconfig_bytes(), None);
    let text = std::fs::read_to_string(&xdg).unwrap();
    assert!(text.contains("name = Xdg"), "{text}");
}

#[test]
fn unset_and_same_value() {
    let user = User::new();
    user.global(&["user.name", "Glob"]);
    user.global(&["user.email", "glob@example.com"]);
    let (r, mut s) = user.session();

    // The same values: nothing is written (not even a rewrite of the same bytes).
    let before = user.gitconfig_bytes();
    let modified = std::fs::metadata(user.gitconfig())
        .unwrap()
        .modified()
        .unwrap();
    let out = set(
        &r,
        &mut s,
        json!({"scope": "global", "name": "Glob", "email": "glob@example.com"}),
    )
    .unwrap();
    assert_eq!(out["changed"], false);
    assert_eq!(
        out["previous"],
        json!({"name": "Glob", "email": "glob@example.com"})
    );
    assert_eq!(user.gitconfig_bytes(), before);
    assert_eq!(
        std::fs::metadata(user.gitconfig())
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );

    // An empty string removes the key; the other key stays.
    let out = set(&r, &mut s, json!({"scope": "global", "email": ""})).unwrap();
    assert_eq!(out["email"].to_string(), UNSET);
    assert_eq!(out["global"], json!({"name": "Glob", "email": null}));
    assert_eq!(out["previous"]["email"], "glob@example.com");
    assert_eq!(out["changed"], true);
    let text = String::from_utf8(user.gitconfig_bytes().unwrap()).unwrap();
    assert!(
        !text.contains("email") && text.contains("name = Glob"),
        "{text}"
    );

    // Removing what is not there: not an error, nothing written.
    let before = user.gitconfig_bytes();
    let out = set(&r, &mut s, json!({"scope": "global", "email": ""})).unwrap();
    assert_eq!(out["changed"], false);
    assert_eq!(out["previous"]["email"], Value::Null);
    assert_eq!(user.gitconfig_bytes(), before);

    // The same in a repo, where there was never a local value.
    let repo = Repo::new();
    let config = std::fs::read(repo.path().join(".git/config")).unwrap();
    let (r, mut s) = user.open(&repo);
    let out = set(
        &r,
        &mut s,
        json!({"scope": "local", "name": "", "email": ""}),
    )
    .unwrap();
    assert_eq!(out["changed"], false);
    assert_eq!(
        std::fs::read(repo.path().join(".git/config")).unwrap(),
        config
    );
}

#[test]
fn a_key_with_several_values_becomes_one_and_an_empty_one_can_be_removed() {
    let user = User::new();
    let repo = Repo::new();
    repo.git(&["config", "--add", "user.name", "First"]);
    repo.git(&["config", "--add", "user.name", "Second"]);
    repo.git(&["config", "user.email", ""]);
    let (r, mut s) = user.open(&repo);
    // The last value is the one git uses.
    assert_eq!(get(&r, &mut s)["name"], value("Second", "local"));
    let out = set(
        &r,
        &mut s,
        json!({"scope": "local", "name": "Only", "email": ""}),
    )
    .unwrap();
    // `previous` is the value git used; the earlier one, which the write removes too, is in
    // `previous_all` (only the key that had several).
    assert_eq!(out["previous"], json!({"name": "Second", "email": ""}));
    assert_eq!(out["previous_all"], json!({"name": ["First", "Second"]}));
    assert_eq!(out["local"], json!({"name": "Only", "email": null}));
    assert_eq!(out["changed"], true);
    assert_eq!(local_values(&user, &repo, "user.name"), "Only");
    assert_eq!(local_values(&user, &repo, "user.email"), "");

    // A key written without a value (`[user] name`) reads as empty and can be replaced.
    let config = repo.path().join(".git/config");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("[user]\n\tname\n");
    std::fs::write(&config, text).unwrap();
    assert_eq!(get(&r, &mut s)["name"], value("", "local"));
    let out = set(&r, &mut s, json!({"scope": "local", "name": "Again"})).unwrap();
    assert_eq!(out["previous"]["name"], "");
    assert_eq!(out["previous_all"], json!({"name": ["Only", ""]}));
    assert_eq!(local_values(&user, &repo, "user.name"), "Again");

    // The value asked for is already the last one: nothing is written, nothing is collapsed.
    repo.git(&["config", "--add", "user.name", "Last"]);
    let before = std::fs::read(&config).unwrap();
    let out = set(&r, &mut s, json!({"scope": "local", "name": "Last"})).unwrap();
    assert_eq!(out["changed"], false);
    assert_eq!(out.get("previous_all"), None);
    assert_eq!(std::fs::read(&config).unwrap(), before);
    assert_eq!(local_values(&user, &repo, "user.name"), "Again\nLast");

    // The same for the global file, without a repo; removing the key reports all its values.
    std::fs::write(
        user.gitconfig(),
        "[user]\n\temail = one@example.com\n\temail = two@example.com\n",
    )
    .unwrap();
    let (r, mut s) = user.session();
    let out = set(&r, &mut s, json!({"scope": "global", "email": ""})).unwrap();
    assert_eq!(
        out["previous"],
        json!({"name": null, "email": "two@example.com"})
    );
    assert_eq!(
        out["previous_all"],
        json!({"email": ["one@example.com", "two@example.com"]})
    );
    assert_eq!(out["global"]["email"], Value::Null);
}

#[test]
fn values_are_written_as_they_are() {
    let user = User::new();
    let repo = Repo::new();
    let (r, mut s) = user.open(&repo);
    for (name, email) in [
        ("-x", "--global@example.com"),
        ("--system", "-f"),
        ("  Ada  Lovelace ", " ada@example.com "),
        ("Ad\u{e0} \"Q\" \\ ; # [x] = y", "a+b@ex\u{e4}mple.com"),
        ("\u{6f22}\u{5b57} \u{1f600}", "x@example.com\t"),
    ] {
        let out = set(
            &r,
            &mut s,
            json!({"scope": "local", "name": name, "email": email}),
        )
        .unwrap();
        assert_eq!(out["local"], json!({"name": name, "email": email}));
        assert_eq!(out["name"], value(name, "local"));
        // git reads back exactly the same text.
        let (ok, read) = user.git_in(
            repo.path(),
            &["config", "--local", "-z", "--get-all", "user.name"],
        );
        assert!(ok);
        assert_eq!(read, format!("{name}\0"));
        let (_, read) = user.git_in(
            repo.path(),
            &["config", "--local", "-z", "--get-all", "user.email"],
        );
        assert_eq!(read, format!("{email}\0"));
    }
    // Nothing else of the file was touched, and no global file appeared.
    let (_, bare) = user.git_in(repo.path(), &["config", "--local", "core.bare"]);
    assert_eq!(bare, "false");
    assert_eq!(user.gitconfig_bytes(), None);
}

#[test]
fn invalid_values_are_refused_and_nothing_is_written() {
    let user = User::new();
    let repo = Repo::new();
    let config = std::fs::read(repo.path().join(".git/config")).unwrap();
    let (r, mut s) = user.open(&repo);
    for bad in [
        json!({"scope": "global"}),
        json!({"scope": "local"}),
        json!({"name": "A"}),
        json!({"scope": "system", "name": "A"}),
        json!({"scope": "worktree", "name": "A"}),
        json!({"scope": "global", "name": "A\nB"}),
        json!({"scope": "global", "name": "A\r"}),
        json!({"scope": "local", "email": "a\u{0}b"}),
        json!({"scope": "global", "name": "Ada <ada@example.com>"}),
        json!({"scope": "local", "email": "<ada@example.com>"}),
        json!({"scope": "global", "name": " "}),
        json!({"scope": "global", "name": ".,:;\"'\\"}),
        json!({"scope": "global", "name": "A", "email": "\t"}),
        json!({"scope": "global", "name": "A", "file": "/etc/gitconfig"}),
        json!({"scope": "global", "name": "A", "system": true}),
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
    assert_eq!(user.gitconfig_bytes(), None);
    assert_eq!(
        std::fs::read(repo.path().join(".git/config")).unwrap(),
        config
    );
}

/// `[user]` then `[include]`: the included file sets the name after the main file.
fn user_with_included_name() -> User {
    let user = User::new();
    std::fs::write(
        user.gitconfig(),
        "[user]\n\tname = Main\n\temail = main@example.com\n[include]\n\tpath = extra.cfg\n",
    )
    .unwrap();
    std::fs::write(user.home().join("extra.cfg"), "[user]\n\tname = Included\n").unwrap();
    user
}

fn decided_elsewhere(err: &EngineError, file: &str, value: &str) {
    assert!(
        matches!(err, EngineError::NotAllowed(m) if m.key() == "engine-config-decided-elsewhere"),
        "{err}"
    );
    let text = err.to_string();
    assert!(text.contains("nothing was written"), "{text}");
    assert!(text.contains(file), "{file}: {text}");
    assert!(text.contains(&format!("“{value}”")), "{value}: {text}");
}

#[test]
fn an_included_file_keeps_the_scope_of_the_file_that_includes_it() {
    let user = user_with_included_name();
    let repo = Repo::new();
    std::fs::write(
        repo.path().join(".git/local-extra.cfg"),
        "[user]\n\temail = inc@example.com\n",
    )
    .unwrap();
    repo.git(&["config", "include.path", "local-extra.cfg"]);

    let (r, mut s) = user.open(&repo);
    let out = get(&r, &mut s);
    assert_eq!(out["name"], value("Included", "global"));
    assert_eq!(out["global"]["name"], "Included");
    assert_eq!(out["email"], value("inc@example.com", "local"));
    assert_eq!(
        out["local"],
        json!({"name": null, "email": "inc@example.com"})
    );
    assert_eq!(out["global"]["email"], "main@example.com");
}

/// The blocker of the review: a write to the main file must not replace its own value when an
/// included file decides what git reads, and must say which file that is.
#[test]
fn a_key_decided_by_an_included_file_is_refused_before_anything_is_written() {
    let user = user_with_included_name();
    let main = user.gitconfig_bytes();
    let extra = user.home().join("extra.cfg");
    let extra_name = extra.to_str().unwrap().to_owned();
    let repo = Repo::new();
    std::fs::write(
        repo.path().join(".git/local-extra.cfg"),
        "[user]\n\temail = inc@example.com\n",
    )
    .unwrap();
    repo.git(&["config", "include.path", "local-extra.cfg"]);
    let local = std::fs::read(repo.path().join(".git/config")).unwrap();

    for (r, mut s) in [user.session(), user.open(&repo)] {
        // Another value, removing the key, and together with a key that could be written.
        for params in [
            json!({"scope": "global", "name": "Wanted"}),
            json!({"scope": "global", "name": ""}),
            json!({"scope": "global", "name": "Wanted", "email": "new@example.com"}),
        ] {
            let err = set(&r, &mut s, params).unwrap_err();
            decided_elsewhere(&err, &extra_name, "Included");
            assert!(err.to_string().contains("user.name"), "{err}");
        }
        // The value the included file already gives: nothing to write.
        let out = set(&r, &mut s, json!({"scope": "global", "name": "Included"})).unwrap();
        assert_eq!(out["changed"], false);
        // `previous` is what the main file holds, not what the included one gives.
        assert_eq!(
            out["previous"],
            json!({"name": "Main", "email": "main@example.com"})
        );
        assert_eq!(
            user.gitconfig_bytes(),
            main,
            "the main file was not written"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&extra).unwrap(),
        "[user]\n\tname = Included\n"
    );

    // The local file includes one that sets the email: the same for the local scope, whether
    // the file itself has a value or not.
    let (r, mut s) = user.open(&repo);
    for email in ["", "loc@example.com"] {
        let err = set(&r, &mut s, json!({"scope": "local", "email": email})).unwrap_err();
        decided_elsewhere(&err, "local-extra.cfg", "inc@example.com");
    }
    assert_eq!(
        std::fs::read(repo.path().join(".git/config")).unwrap(),
        local
    );

    // A key the main file decides is written as usual, next to one that is already in effect.
    let out = set(
        &r,
        &mut s,
        json!({"scope": "global", "name": "Included", "email": "new@example.com"}),
    )
    .unwrap();
    assert_eq!(out["changed"], true);
    assert_eq!(
        out["previous"],
        json!({"name": "Main", "email": "main@example.com"})
    );
    assert_eq!(
        out["global"],
        json!({"name": "Included", "email": "new@example.com"})
    );
    let text = String::from_utf8(user.gitconfig_bytes().unwrap()).unwrap();
    assert!(
        text.contains("name = Main") && text.contains("email = new@example.com"),
        "{text}"
    );
}

/// The scenario of the review: a personal email in `~/.gitconfig`, a work one included only for
/// the repositories under `~/work/`.
#[test]
fn a_conditional_include_decides_only_in_its_repositories() {
    let user = User::new();
    // git compares `gitdir:` with the real path of the repository.
    let home = user.home().canonicalize().unwrap();
    let work = home.join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(
        user.gitconfig(),
        format!(
            "[user]\n\tname = Ada\n\temail = personal@example.com\n[includeIf \"gitdir:{}/\"]\n\tpath = work.cfg\n",
            work.to_str().unwrap()
        ),
    )
    .unwrap();
    let work_cfg = home.join("work.cfg");
    std::fs::write(&work_cfg, "[user]\n\temail = work@corp.example\n").unwrap();
    let main = user.gitconfig_bytes();
    let proj = work.join("proj");
    let (ok, _) = user.git_in(&home, &["init", "--quiet", proj.to_str().unwrap()]);
    assert!(ok);

    // In the work repository git reads the work email, in the global scope.
    let (r, mut s) = user.session();
    r.run(
        "repo.open",
        &mut s,
        json!({ "path": proj.to_str().unwrap() }),
    )
    .unwrap();
    let out = get(&r, &mut s);
    assert_eq!(out["email"], value("work@corp.example", "global"));
    assert_eq!(out["global"]["email"], "work@corp.example");
    // Setting the global email from there would replace the personal one, used by every other
    // repository, without changing what this repository reads: refused, nothing written.
    let err = set(
        &r,
        &mut s,
        json!({"scope": "global", "email": "new@example.com"}),
    )
    .unwrap_err();
    decided_elsewhere(&err, work_cfg.to_str().unwrap(), "work@corp.example");
    assert_eq!(user.gitconfig_bytes(), main);
    // The name is the main file's: it can be changed from here.
    let out = set(&r, &mut s, json!({"scope": "global", "name": "Ada L."})).unwrap();
    assert_eq!(out["previous"]["name"], "Ada");
    assert_eq!(out["previous"]["email"], "personal@example.com");
    // The local scope is always available.
    let out = set(
        &r,
        &mut s,
        json!({"scope": "local", "email": "proj@example.com"}),
    )
    .unwrap();
    assert_eq!(out["email"], value("proj@example.com", "local"));

    // With no repo open, and in a repository elsewhere, the include does not apply: the
    // personal email is the global one and is what a write replaces and reports.
    let other = Repo::new();
    for (r, mut s) in [user.session(), user.open(&other)] {
        assert_eq!(
            get(&r, &mut s)["email"],
            value("personal@example.com", "global")
        );
    }
    let (r, mut s) = user.session();
    let out = set(
        &r,
        &mut s,
        json!({"scope": "global", "email": "new@example.com"}),
    )
    .unwrap();
    assert_eq!(out["previous"]["email"], "personal@example.com");
    assert_eq!(out["email"], value("new@example.com", "global"));
    assert_eq!(
        std::fs::read_to_string(&work_cfg).unwrap(),
        "[user]\n\temail = work@corp.example\n"
    );
}

/// `~/.gitconfig` and the XDG file both exist: git reads both, `--global` writes the first. A
/// key only the XDG file sets is decided "elsewhere": refused, naming that file (git would
/// read a value written to `~/.gitconfig` after it, but the command does not guess).
#[test]
fn a_key_of_the_xdg_file_is_not_written_into_the_home_file() {
    let user = User::new();
    let xdg = user.xdg().join("git/config");
    std::fs::create_dir_all(xdg.parent().unwrap()).unwrap();
    std::fs::write(&xdg, "[user]\n\temail = xdg@example.com\n").unwrap();
    std::fs::write(user.gitconfig(), "[user]\n\tname = Home\n").unwrap();
    let (r, mut s) = user.session();
    let out = get(&r, &mut s);
    assert_eq!(
        out["global"],
        json!({"name": "Home", "email": "xdg@example.com"})
    );
    let err = set(
        &r,
        &mut s,
        json!({"scope": "global", "email": "new@example.com"}),
    )
    .unwrap_err();
    decided_elsewhere(&err, xdg.to_str().unwrap(), "xdg@example.com");
    let out = set(&r, &mut s, json!({"scope": "global", "name": "New"})).unwrap();
    assert_eq!(out["previous"], json!({"name": "Home", "email": null}));
    assert_eq!(
        std::fs::read_to_string(&xdg).unwrap(),
        "[user]\n\temail = xdg@example.com\n"
    );
}

#[test]
fn the_worktree_file_is_another_scope() {
    let user = User::new();
    user.global(&["user.name", "Glob"]);
    let repo = Repo::new();
    repo.git(&["config", "extensions.worktreeConfig", "true"]);
    repo.git(&["config", "--worktree", "user.name", "Tree"]);
    let (r, mut s) = user.open(&repo);
    let out = get(&r, &mut s);
    assert_eq!(out["name"], value("Tree", "other"));
    assert_eq!(out["local"]["name"], Value::Null);
    assert_eq!(out["global"]["name"], "Glob");
    // A local write succeeds (the local scope holds the value); the result shows what wins.
    let out = set(&r, &mut s, json!({"scope": "local", "name": "Loc"})).unwrap();
    assert_eq!(out["local"]["name"], "Loc");
    assert_eq!(out["name"], value("Tree", "other"));
    assert_eq!(out["changed"], true);
}

#[test]
fn environment_overrides_are_listed() {
    let user = User::new();
    user.global(&["user.name", "Glob"]);
    let repo = Repo::new();
    let (r, mut s) = user.session_with(
        user.opener()
            .env("GIT_AUTHOR_NAME", "Env Author")
            .env("GIT_COMMITTER_EMAIL", "")
            .env("EMAIL", "env@example.com"),
    );
    // No `user.email`: `EMAIL` is what git would use.
    let out = get(&r, &mut s);
    assert_eq!(
        out["env"],
        json!(["GIT_AUTHOR_NAME", "GIT_COMMITTER_EMAIL", "EMAIL"])
    );
    assert_eq!(
        out["name"],
        value("Glob", "global"),
        "the configuration is reported as it is"
    );

    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    assert_eq!(get(&r, &mut s)["env"], out["env"]);
    // With an email configured `EMAIL` no longer counts; the result of the write says so too.
    let out = set(
        &r,
        &mut s,
        json!({"scope": "local", "email": "loc@example.com"}),
    )
    .unwrap();
    assert_eq!(
        out["env"],
        json!(["GIT_AUTHOR_NAME", "GIT_COMMITTER_EMAIL"])
    );

    // The opener of the other tests inherits none of them.
    let (r, mut s) = user.session();
    assert_eq!(get(&r, &mut s)["env"], json!([]));
}

#[test]
fn a_commit_without_identity_is_recognised_and_works_after_setting_it() {
    let user = User::new();
    // No guessing from the system user and host name: the failure is the same everywhere.
    user.global(&["user.useConfigOnly", "true"]);
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.stage(&["a.txt"]);
    let (r, mut s) = user.open(&repo);
    let err = r
        .run("commit.create", &mut s, json!({"message": "first"}))
        .unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");
    assert!(err.is_missing_identity(), "{err}");
    assert!(
        project_engine::is_missing_identity(&err.to_string()),
        "{err}"
    );
    // Another failure of the same command is not taken for it.
    let other = r
        .run("commit.create", &mut s, json!({"message": ""}))
        .unwrap_err();
    assert!(!other.is_missing_identity(), "{other}");

    // Only a name: git still has no email.
    set(&r, &mut s, json!({"scope": "global", "name": "Glob Al"})).unwrap();
    let err = r
        .run("commit.create", &mut s, json!({"message": "first"}))
        .unwrap_err();
    assert!(err.is_missing_identity(), "{err}");

    set(
        &r,
        &mut s,
        json!({"scope": "local", "email": "loc@example.com"}),
    )
    .unwrap();
    r.run("commit.create", &mut s, json!({"message": "first"}))
        .unwrap();
    let (_, who) = user.git_in(repo.path(), &["log", "-1", "--format=%an <%ae> %cn <%ce>"]);
    assert_eq!(who, "Glob Al <loc@example.com> Glob Al <loc@example.com>");
}

#[test]
fn a_locked_file_fails_with_the_text_of_git_and_changes_nothing() {
    let user = User::new();
    user.global(&["user.name", "Glob"]);
    let before = user.gitconfig_bytes();
    // Another git is writing the file (or left its lock behind).
    let lock = user.home().join(".gitconfig.lock");
    std::fs::write(&lock, "").unwrap();
    let (r, mut s) = user.session();
    let err = set(
        &r,
        &mut s,
        json!({"scope": "global", "name": "New", "email": "new@example.com"}),
    )
    .unwrap_err();
    assert!(
        matches!(&err, EngineError::Git { command, stderr } if command == "config" && stderr.contains("could not lock config file")),
        "{err}"
    );
    assert_eq!(user.gitconfig_bytes(), before);
    assert_eq!(
        std::fs::read(&lock).unwrap(),
        b"",
        "the lock is not ours to remove"
    );
    // Reads do not need the lock.
    assert_eq!(get(&r, &mut s)["name"], value("Glob", "global"));
    std::fs::remove_file(&lock).unwrap();

    // The same for the repo file.
    let repo = Repo::new();
    let config = std::fs::read(repo.path().join(".git/config")).unwrap();
    let lock = repo.path().join(".git/config.lock");
    std::fs::write(&lock, "").unwrap();
    let (r, mut s) = user.open(&repo);
    let err = set(&r, &mut s, json!({"scope": "local", "name": "New"})).unwrap_err();
    assert!(
        matches!(&err, EngineError::Git { stderr, .. } if stderr.contains("could not lock config file")),
        "{err}"
    );
    assert_eq!(
        std::fs::read(repo.path().join(".git/config")).unwrap(),
        config
    );
    assert_eq!(get(&r, &mut s)["local"]["name"], Value::Null);
}

#[test]
fn without_a_home_the_global_write_is_refused_by_git() {
    let user = User::new();
    if cfg!(windows) {
        // Git for Windows finds a home by other means.
        return;
    }
    // Not `User::opener`: no `HOME` at all (and still none of the real configuration).
    let mut bare = CliOpener::new().env("GIT_CONFIG_NOSYSTEM", "1");
    for name in ["HOME", "XDG_CONFIG_HOME", "GIT_CONFIG_GLOBAL"]
        .into_iter()
        .chain(IDENTITY_ENV)
    {
        bare = bare.env_remove(name);
    }
    let (r, mut s) = user.session_with(bare);
    let err = set(&r, &mut s, json!({"scope": "global", "name": "New"})).unwrap_err();
    assert!(
        matches!(&err, EngineError::Git { command, stderr } if command == "config" && stderr.contains("HOME")),
        "{err}"
    );
    assert_eq!(user.gitconfig_bytes(), None);
    // A repo can still be given its own identity.
    let repo = Repo::new();
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    let out = set(&r, &mut s, json!({"scope": "local", "name": "Loc"})).unwrap();
    assert_eq!(out["name"], value("Loc", "local"));
}

/// The identity is what a stopped merge needs to be concluded: setting it does not depend on
/// the state of the repo and does not touch it.
#[test]
fn works_during_a_merge_and_with_a_detached_head() {
    let user = User::new();
    let repo = common::repo_with_merge_conflict();
    let (r, mut s) = user.open(&repo);
    let operation = r.run("repo.operation", &mut s, json!({})).unwrap();
    assert_eq!(operation["operation"], "Merge", "{operation}");
    let status = r.run("repo.status", &mut s, json!({})).unwrap();
    let out = set(
        &r,
        &mut s,
        json!({"scope": "local", "name": "Loc", "email": "loc@example.com"}),
    )
    .unwrap();
    assert_eq!(out["changed"], true);
    assert_eq!(
        r.run("repo.operation", &mut s, json!({})).unwrap(),
        operation
    );
    assert_eq!(r.run("repo.status", &mut s, json!({})).unwrap(), status);

    let detached = common::repo_detached_head();
    let (r, mut s) = user.open(&detached.repo);
    let head = r.run("repo.status", &mut s, json!({})).unwrap()["head"].clone();
    let out = set(&r, &mut s, json!({"scope": "local", "name": "Loc"})).unwrap();
    assert_eq!(out["name"], value("Loc", "local"));
    assert_eq!(
        r.run("repo.status", &mut s, json!({})).unwrap()["head"],
        head
    );
}

#[test]
fn the_opener_and_the_backend_agree() {
    let user = User::new();
    user.global(&["user.email", "glob@example.com"]);
    let repo = Repo::new();
    repo.git(&["config", "user.name", "Loc"]);
    let opener = user.opener();
    let outside = opener.identity().unwrap();
    assert_eq!(outside.email.value.as_deref(), Some("glob@example.com"));
    assert_eq!(outside.name.value, None);
    let backend = opener.open_cli(repo.path_str()).unwrap();
    let inside = project_engine::GitConfig::identity(&backend).unwrap();
    assert_eq!(inside.global, outside.global);
    assert_eq!(inside.local.name.as_deref(), Some("Loc"));
    assert_eq!(inside.configured(), Some(("Loc", "glob@example.com")));
}
