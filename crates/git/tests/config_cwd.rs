//! `config.identity` and `config.set_identity` without an open repo, when the process itself was
//! started inside a repository (`project-cli` run from a checkout): the repo around the current
//! directory is not "the open repo". A separate file with a single test because it changes the
//! current directory of the process. The global file is in a temporary `HOME`, never the real
//! one (see `config.rs`).

mod common;

use common::Repo;
use project_engine::{EngineError, IDENTITY_ENV, Registry, Session};
use project_git::CliOpener;
use serde_json::{Value, json};
use std::sync::Arc;

#[test]
fn the_repo_around_the_current_directory_is_not_read_or_written() {
    let home = tempfile::tempdir().unwrap();
    let repo = Repo::new();
    repo.git(&["config", "user.name", "Cwd Local"]);
    repo.git(&["config", "user.email", "cwd@example.com"]);
    repo.git(&["config", "extensions.worktreeConfig", "true"]);
    repo.git(&["config", "--worktree", "user.email", "tree@example.com"]);
    let config = std::fs::read(repo.path().join(".git/config")).unwrap();
    std::env::set_current_dir(repo.path()).unwrap();

    let opener = || {
        let mut opener = CliOpener::new()
            .env_remove("GIT_CONFIG_GLOBAL")
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("xdg"))
            .env("GIT_CONFIG_NOSYSTEM", "1");
        for name in IDENTITY_ENV {
            opener = opener.env_remove(name);
        }
        opener
    };
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(opener()));

    let out = r.run("config.identity", &mut s, json!({})).unwrap();
    assert_eq!(out["name"], json!({"value": null, "scope": null}), "{out}");
    assert_eq!(out["email"], json!({"value": null, "scope": null}));
    assert_eq!(out["local"], json!({"name": null, "email": null}));

    // Local is refused even though git would find a repo here.
    let err = r
        .run(
            "config.set_identity",
            &mut s,
            json!({"scope": "local", "name": "X"}),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-config-local-no-repo"),
        "{err}"
    );

    let out = r
        .run(
            "config.set_identity",
            &mut s,
            json!({"scope": "global", "name": "Glob", "email": "glob@example.com"}),
        )
        .unwrap();
    assert_eq!(out["name"], json!({"value": "Glob", "scope": "global"}));
    assert_eq!(out["local"], json!({"name": null, "email": null}));
    assert_eq!(
        out["previous"],
        json!({"name": Value::Null, "email": Value::Null})
    );
    assert!(home.path().join(".gitconfig").is_file());
    assert_eq!(
        std::fs::read(repo.path().join(".git/config")).unwrap(),
        config
    );

    // Opening that repo shows its own values over the global ones.
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    let out = r.run("config.identity", &mut s, json!({})).unwrap();
    assert_eq!(out["name"], json!({"value": "Cwd Local", "scope": "local"}));
    assert_eq!(
        out["email"],
        json!({"value": "tree@example.com", "scope": "other"})
    );
    assert_eq!(out["global"]["name"], "Glob");

    // A conditional include that applies to the repo around the current directory does not
    // apply to "no repo open": the global email is the one of the main file, and that is the
    // one a write replaces and reports.
    let (r, mut s) = (Registry::builtin(), Session::new(Arc::new(opener())));
    let real = repo.path().canonicalize().unwrap();
    let gitconfig = home.path().join(".gitconfig");
    let mut text = std::fs::read_to_string(&gitconfig).unwrap();
    text.push_str(&format!(
        "[includeIf \"gitdir:{}/\"]\n\tpath = work.cfg\n",
        real.to_str().unwrap()
    ));
    std::fs::write(&gitconfig, text).unwrap();
    std::fs::write(
        home.path().join("work.cfg"),
        "[user]\n\temail = work@corp.example\n",
    )
    .unwrap();
    assert_eq!(
        std::env::current_dir().unwrap().canonicalize().unwrap(),
        real
    );
    let out = r.run("config.identity", &mut s, json!({})).unwrap();
    assert_eq!(
        out["email"],
        json!({"value": "glob@example.com", "scope": "global"}),
        "{out}"
    );
    let out = r
        .run(
            "config.set_identity",
            &mut s,
            json!({"scope": "global", "email": "new@example.com"}),
        )
        .unwrap();
    assert_eq!(out["previous"]["email"], "glob@example.com");
    assert_eq!(
        out["email"],
        json!({"value": "new@example.com", "scope": "global"})
    );
    // Opened, that repo reads the included email in the global scope (its worktree file
    // still wins overall).
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    let out = r.run("config.identity", &mut s, json!({})).unwrap();
    assert_eq!(out["global"]["email"], "work@corp.example");

    // A repo with a configuration git cannot parse around the current directory: with no repo
    // open it is not read at all, so the global identity can still be read and written.
    let broken = Repo::new();
    let mut text = std::fs::read_to_string(broken.path().join(".git/config")).unwrap();
    text.push_str("garbage [[[\n");
    std::fs::write(broken.path().join(".git/config"), &text).unwrap();
    std::env::set_current_dir(broken.path()).unwrap();
    let (r, mut s) = (Registry::builtin(), Session::new(Arc::new(opener())));
    let out = r.run("config.identity", &mut s, json!({})).unwrap();
    assert_eq!(
        out["name"],
        json!({"value": "Glob", "scope": "global"}),
        "{out}"
    );
    let out = r
        .run(
            "config.set_identity",
            &mut s,
            json!({"scope": "global", "name": "Still Works"}),
        )
        .unwrap();
    assert_eq!(out["previous"]["name"], "Glob");
    assert_eq!(out["name"]["value"], "Still Works");
    assert_eq!(
        std::fs::read_to_string(broken.path().join(".git/config")).unwrap(),
        text
    );
    // Opening it is another matter: git's error.
    assert!(
        r.run("repo.open", &mut s, json!({ "path": broken.path_str() }))
            .is_err()
    );

    // Leave the folder before the temporary repo is deleted.
    std::env::set_current_dir(home.path()).unwrap();
}
