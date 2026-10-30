//! Integration tests of remote management (S27: `remote.list`, `remote.add`,
//! `remote.remove`) with the `CliBackend` on temporary repos and local remotes (`git init --bare`):
//! URLs with masked credentials, add with fetch and undo, rejected names and URLs (`-x`,
//! `ext::`), removal without confirmation (nothing changes), with confirmation and undo that
//! restores configuration, remote branches, symbolic `HEAD` and upstream, `remote.pushDefault`,
//! nested remotes (`fork` and `fork/a`) and undo conflicts.

mod common;

use common::Repo;
use project_engine::{EngineError, GitRemotes, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn open(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// The repo configuration and all the refs (symbolic ones too), to check that nothing changes.
fn state(repo: &Repo) -> (String, String) {
    let config = std::fs::read_to_string(repo.path().join(".git/config")).unwrap();
    let refs = repo.git(&[
        "for-each-ref",
        "--format=%(refname) %(objectname) %(symref)",
    ]);
    (config, refs)
}

/// The local configuration entries, sorted (undo may rewrite them in a different order).
fn local_config(repo: &Repo) -> Vec<String> {
    let mut lines: Vec<String> = repo
        .git(&["config", "--local", "--list"])
        .lines()
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

fn remote_refs(repo: &Repo, remote: &str) -> String {
    repo.git(&[
        "for-each-ref",
        "--format=%(refname) %(objectname) %(symref)",
        &format!("refs/remotes/{remote}/"),
    ])
}

/// A bare remote with `main` and `feature`, and the repo under test that has it as `origin` with
/// everything fetched, symbolic `origin/HEAD` and `main`/`feature` following the remote branches.
struct Setup {
    remote: Repo,
    work: Repo,
}

fn setup() -> Setup {
    let remote = Repo::bare();
    let work = Repo::new();
    work.commit_file("a.txt", "one\n", "first");
    work.add_remote("origin", &remote);
    work.push("origin", "HEAD", "main");
    work.branch("feature");
    work.push("origin", "feature", "feature");
    work.set_upstream("main", "origin");
    work.set_upstream("feature", "origin");
    work.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    Setup { remote, work }
}

#[test]
fn list_with_several_remotes_masks_the_credentials() {
    let Setup { remote: _r, work } = setup();
    work.git(&[
        "remote",
        "add",
        "tok",
        "https://nicola:secret@example.com/r.git",
    ]);
    work.git(&[
        "remote",
        "add",
        "gh",
        "https://ghp_abc123@github.com/x/y.git",
    ]);
    work.git(&[
        "remote",
        "add",
        "-m",
        "dev",
        "scp",
        "git@example.com:a/b.git",
    ]);
    work.git(&[
        "config",
        "remote.origin.pushurl",
        "https://bob:other-secret@example.com/push.git",
    ]);
    work.git(&["config", "remote.pushDefault", "tok"]);
    let (r, mut s) = open(&work);
    let out = run(&r, &mut s, "remote.list", json!({}));
    let text = out.to_string();
    for secret in ["secret", "ghp_abc123", "other-secret"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    let names: Vec<&str> = out["remotes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["gh", "origin", "scp", "tok"]);
    let [gh, origin, scp, tok] = &out["remotes"].as_array().unwrap()[..] else {
        panic!("{out}");
    };
    assert_eq!(gh["url"], "https://***@github.com/x/y.git");
    assert_eq!(gh["credentials"], true);
    assert_eq!(origin["url"], work_url(&out));
    assert_eq!(
        origin["push_urls"],
        json!(["https://bob:***@example.com/push.git"])
    );
    assert_eq!(origin["credentials"], true);
    assert_eq!(origin["branches"], 2, "symbolic origin/HEAD does not count");
    assert_eq!(origin["default"], true);
    assert_eq!(
        origin["fetch"],
        json!(["+refs/heads/*:refs/remotes/origin/*"])
    );
    assert_eq!(scp["url"], "git@example.com:a/b.git");
    assert_eq!(scp["credentials"], false);
    assert_eq!(scp["fetch"], json!(["+refs/heads/*:refs/remotes/scp/*"]));
    assert_eq!(tok["url"], "https://nicola:***@example.com/r.git");
    assert_eq!(tok["push_default"], true);
    assert_eq!(tok["branches"], 0);
    assert_eq!(out["default"], "origin");
    assert_eq!(out["push_default"], "tok");
}

/// The URL of `origin` as given by `remote.list` (the path of the bare remote).
fn work_url(out: &Value) -> Value {
    out["remotes"][1]["url"].clone()
}

#[test]
fn add_with_fetch_then_undo_and_redo() {
    let Setup { remote, work } = setup();
    let (r, mut s) = open(&work);
    let out = run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "copy", "url": remote.path_str(), "fetch": true }),
    );
    assert_eq!(out["fetched"], true, "{out}");
    assert_eq!(out["fetch_error"], Value::Null);
    let updated: Vec<&str> = out["fetch"]["updated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["name"].as_str().unwrap())
        .collect();
    assert!(updated.contains(&"refs/remotes/copy/main"), "{updated:?}");
    assert!(
        updated.contains(&"refs/remotes/copy/feature"),
        "{updated:?}"
    );
    assert_eq!(
        work.git(&["config", "remote.copy.fetch"]),
        "+refs/heads/*:refs/remotes/copy/*"
    );
    assert_eq!(work.git(&["config", "remote.copy.url"]), remote.path_str());
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Add remote `copy` (with fetch")
    );

    run(&r, &mut s, "edit.undo", json!({}));
    assert!(remote_refs(&work, "copy").is_empty());
    assert!(!work.git(&["remote"]).lines().any(|l| l == "copy"));
    // `origin` is untouched.
    assert_eq!(remote_refs(&work, "origin").lines().count(), 3);

    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(work.git(&["config", "remote.copy.url"]), remote.path_str());
    assert!(
        remote_refs(&work, "copy").contains("refs/remotes/copy/main"),
        "{}",
        remote_refs(&work, "copy")
    );
}

#[test]
fn add_without_fetch_and_undo_refused_after_a_fetch() {
    let Setup { remote, work } = setup();
    let (r, mut s) = open(&work);
    let out = run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "fork", "url": remote.path_str() }),
    );
    assert_eq!(out["fetched"], false);
    assert!(remote_refs(&work, "fork").is_empty());
    // A fetch after the add changes the remote: undo does not delete the fetched branches.
    run(&r, &mut s, "remote.fetch", json!({ "remote": "fork" }));
    let before = state(&work);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(state(&work), before);
}

#[test]
fn add_of_an_existing_name_changes_nothing() {
    let Setup { remote, work } = setup();
    let (r, mut s) = open(&work);
    let before = state(&work);
    let err = r
        .run(
            "remote.add",
            &mut s,
            json!({ "name": "origin", "url": "/other" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("already exists"), "{err}");
    assert_eq!(state(&work), before);
    // Even if it reaches the backend (git check).
    let mut backend = work.open();
    let err = backend
        .create_remote("origin", remote.path_str())
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(state(&work), before);
}

#[test]
fn invalid_names_and_urls_never_reach_git() {
    let Setup { remote, work } = setup();
    let (r, mut s) = open(&work);
    let before = state(&work);
    let marker = work.path().join("taken");
    let ext = format!("ext::sh -c touch% {}", marker.display());
    for (name, url) in [
        ("-x", remote.path_str()),
        ("--upload-pack=touch", remote.path_str()),
        ("a b", remote.path_str()),
        ("a..b", remote.path_str()),
        ("", remote.path_str()),
        ("x", "-x"),
        ("x", "--upload-pack=touch /tmp/x"),
        ("x", ext.as_str()),
        ("x", "ext://sh"),
        ("x", "fd::3"),
        ("x", "ssh://-oProxyCommand=touch/r"),
        ("x", "git@-oProxyCommand=touch:r"),
        ("x", ""),
        ("x", "/srv/r\n[core]"),
    ] {
        let err = r
            .run(
                "remote.add",
                &mut s,
                json!({ "name": name, "url": url, "fetch": true }),
            )
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{name} {url}: {err}"
        );
    }
    assert!(!marker.exists());
    assert_eq!(state(&work), before);

    // A remote named `-x` (created by git after `--end-of-options`) is not an option:
    // the backend creates it as a name, and `remote.remove` removes it.
    let mut backend = work.open();
    backend.create_remote("-x", remote.path_str()).unwrap();
    assert_eq!(work.git(&["config", "remote.-x.url"]), remote.path_str());
    run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "-x", "confirm": true }),
    );
    assert_eq!(state(&work), before);
}

#[test]
fn remove_without_confirm_changes_nothing() {
    let Setup { remote: _r, work } = setup();
    work.git(&["config", "branch.feature.pushRemote", "origin"]);
    let (r, mut s) = open(&work);
    let before = state(&work);
    let err = r
        .run("remote.remove", &mut s, json!({ "name": "origin" }))
        .unwrap_err();
    let EngineError::ConfirmRequired { reason, .. } = &err else {
        panic!("{err}");
    };
    assert!(reason.contains("2 remote-tracking branches"), "{reason}");
    assert!(reason.contains("`feature`, `main`"), "{reason}");
    assert!(reason.contains("pushRemote"), "{reason}");
    assert_eq!(state(&work), before);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["can_undo"], false);
}

#[test]
fn remove_with_confirm_then_undo_restores_config_refs_and_upstreams() {
    let Setup { remote, work } = setup();
    // A commit that exists only on `origin/only`: after the removal it stays in the backup.
    let other = Repo::new();
    other.add_remote("origin", &remote);
    other.git(&["fetch", "--quiet", "origin"]);
    other.git(&["checkout", "--quiet", "-b", "only", "origin/main"]);
    let only = other.commit_file("b.txt", "two\n", "only on the remote");
    other.git(&["push", "--quiet", "origin", "only"]);
    work.git(&["fetch", "--quiet", "origin"]);
    work.git(&["config", "branch.feature.pushRemote", "origin"]);
    work.git(&["config", "remote.origin.tagOpt", "--no-tags"]);
    let config = local_config(&work);
    let refs = remote_refs(&work, "origin");
    let (r, mut s) = open(&work);

    let out = run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    assert_eq!(out["upstreams_removed"], json!(["feature", "main"]));
    assert_eq!(out["push_remotes_removed"], json!(["feature"]));
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(out["remotes"], json!([]));
    assert_eq!(out["removed"]["head"], "refs/remotes/origin/main");
    let backups = out["backups"].as_array().unwrap();
    assert_eq!(backups.len(), 1, "{out}");
    let backup = backups[0].as_str().unwrap();
    assert!(
        backup.starts_with("refs/project/backup/remote-"),
        "{backup}"
    );
    assert_eq!(work.rev(backup), only);
    assert!(remote_refs(&work, "origin").is_empty());
    assert!(work.git(&["remote"]).is_empty());
    let left = local_config(&work);
    assert!(
        !left.iter().any(|l| l.starts_with("remote.origin.")),
        "{left:?}"
    );
    assert!(!left.iter().any(|l| l.starts_with("branch.")), "{left:?}");

    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["warnings"], json!([]), "{undo}");
    assert_eq!(local_config(&work), config);
    assert_eq!(remote_refs(&work, "origin"), refs);
    assert_eq!(
        work.git(&["rev-parse", "--abbrev-ref", "main@{upstream}"]),
        "origin/main"
    );

    // Redo: removed again, without a new backup; then undo again.
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(redo["backups"], json!([]));
    assert!(remote_refs(&work, "origin").is_empty());
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(local_config(&work), config);
    assert_eq!(remote_refs(&work, "origin"), refs);
}

#[test]
fn remove_a_remote_used_as_push_default() {
    let Setup { remote, work } = setup();
    work.add_remote("fork", &remote);
    work.git(&["fetch", "--quiet", "fork"]);
    work.git(&["config", "remote.pushDefault", "fork"]);
    let config = local_config(&work);
    let (r, mut s) = open(&work);
    let err = r
        .run("remote.remove", &mut s, json!({ "name": "fork" }))
        .unwrap_err();
    assert!(err.to_string().contains("remote.pushDefault"), "{err}");
    let out = run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "fork", "confirm": true }),
    );
    assert_eq!(out["push_default_removed"], true);
    assert_eq!(out["upstreams_removed"], json!([]));
    assert_eq!(out["backups"], json!([]), "the commits are also on main");
    assert!(
        work.git_output(&["config", "remote.pushDefault"])
            .status
            .code()
            == Some(1)
    );
    // Upstream of `main` towards `origin`: untouched.
    assert_eq!(
        work.git(&["rev-parse", "--abbrev-ref", "main@{upstream}"]),
        "origin/main"
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(local_config(&work), config);

    // With another `remote.pushDefault` set in the meantime, undo leaves it and warns.
    run(&r, &mut s, "edit.redo", json!({}));
    work.git(&["config", "remote.pushDefault", "origin"]);
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    let warnings = undo["warnings"].to_string();
    assert!(warnings.contains("remote.pushDefault"), "{warnings}");
    assert_eq!(work.git(&["config", "remote.pushDefault"]), "origin");
    assert_eq!(work.git(&["config", "remote.fork.url"]), remote.path_str());
}

#[test]
fn undo_keeps_upstreams_set_meanwhile_and_refuses_a_recreated_remote() {
    let Setup { remote, work } = setup();
    let (r, mut s) = open(&work);
    run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    // `main` gets another upstream from a terminal: undo does not overwrite it.
    work.add_remote("other", &remote);
    work.git(&["fetch", "--quiet", "other"]);
    work.set_upstream("main", "other");
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    let warnings = undo["warnings"].to_string();
    assert!(warnings.contains("`main`"), "{warnings}");
    assert_eq!(
        work.git(&["rev-parse", "--abbrev-ref", "main@{upstream}"]),
        "other/main"
    );
    assert_eq!(
        work.git(&["rev-parse", "--abbrev-ref", "feature@{upstream}"]),
        "origin/feature"
    );

    // Redo compares and removes only what undo put back: the upstream of `main` towards
    // `other` stays.
    run(&r, &mut s, "edit.redo", json!({}));
    assert!(remote_refs(&work, "origin").is_empty());
    assert_eq!(
        work.git(&["rev-parse", "--abbrev-ref", "main@{upstream}"]),
        "other/main"
    );
    run(&r, &mut s, "edit.undo", json!({}));

    // Removed again and recreated from a terminal with the same name: undo is refused.
    let (r, mut s) = open(&work);
    run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    work.add_remote("origin", &remote);
    let before = state(&work);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(state(&work), before);
}

#[test]
fn nested_remote_names_keep_their_own_refs() {
    let Setup { remote, work } = setup();
    // `git remote add` refuses `origin/a` next to `origin`, but the configuration allows it.
    work.git(&["config", "remote.origin/a.url", remote.path_str()]);
    work.git(&[
        "config",
        "remote.origin/a.fetch",
        "+refs/heads/*:refs/remotes/origin/a/*",
    ]);
    work.git(&["fetch", "--quiet", "origin/a"]);
    let nested = remote_refs(&work, "origin/a");
    assert!(!nested.is_empty());
    let backend = work.open();
    let snapshot = backend.remote_snapshot("origin").unwrap();
    let names: Vec<&str> = snapshot.refs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        ["refs/remotes/origin/feature", "refs/remotes/origin/main"]
    );
    assert_eq!(snapshot.head.as_deref(), Some("refs/remotes/origin/main"));
    assert_eq!(snapshot.upstream_branches(), ["feature", "main"]);
    let (r, mut s) = open(&work);
    run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    assert_eq!(remote_refs(&work, "origin/a"), nested);
    assert_eq!(work.git(&["remote"]), "origin/a");
}

#[test]
fn remove_refuses_unknown_and_global_remotes() {
    let Setup { remote: _r, work } = setup();
    let (r, mut s) = open(&work);
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "missing", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`origin`"), "{err}");

    // A remote defined in an included file: git sees it, but it is not in the repo's file.
    let included = work.path().join("remotes.inc");
    std::fs::write(&included, "[remote \"old\"]\n\turl = /srv/old.git\n").unwrap();
    work.git(&["config", "include.path", included.to_str().unwrap()]);
    let (r, mut s) = open(&work);
    let before = state(&work);
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "old", "confirm": true }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("outside"), "{err}");
    assert_eq!(state(&work), before);
}

/// Session with the global configuration file `<home>/.gitconfig` (in place of `/dev/null`):
/// `GIT_CONFIG_GLOBAL` for git 2.32 and later, `HOME` for older git.
fn open_with_global(repo: &Repo, home: &std::path::Path) -> (Registry, Session) {
    let opener = Repo::opener()
        .env("GIT_CONFIG_GLOBAL", home.join(".gitconfig"))
        .env("HOME", home);
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(opener));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

#[test]
fn remove_refuses_a_remote_defined_partly_elsewhere() {
    let Setup { remote: _r, work } = setup();
    let home = tempfile::tempdir().unwrap();
    let global = home.path().join(".gitconfig");
    std::fs::write(
        &global,
        "[remote \"origin\"]\n\tfetch = +refs/pull/*/head:refs/remotes/origin/pr/*\n",
    )
    .unwrap();
    let (r, mut s) = open_with_global(&work, home.path());
    let before = state(&work);
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "origin", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let text = err.to_string();
    assert!(
        text.contains("remote.origin.fetch") && text.contains("global"),
        "{text}"
    );
    assert_eq!(state(&work), before);

    // Only a `pushurl` in a file included by the repo's file.
    let included = work.path().join("push.inc");
    std::fs::write(
        &included,
        "[remote \"origin\"]\n\tpushurl = /srv/push.git\n",
    )
    .unwrap();
    work.git(&["config", "include.path", included.to_str().unwrap()]);
    let (r, mut s) = open(&work);
    let before = state(&work);
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "origin", "confirm": true }),
        )
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("remote.origin.pushurl") && text.contains("push.inc"),
        "{text}"
    );
    assert_eq!(state(&work), before);
}

#[test]
fn remove_refuses_the_promisor_remote_of_a_partial_clone() {
    let Setup { remote, work: _w } = setup();
    remote.git(&["config", "uploadpack.allowFilter", "true"]);
    let dir = tempfile::tempdir().unwrap();
    let clone = dir.path().join("partial");
    let url = format!("file://{}", remote.path_str());
    let out = std::process::Command::new("git")
        .envs(common::GIT_ENV.iter().copied())
        .args(["clone", "--quiet", "--filter=blob:none", &url])
        .arg(&clone)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run(
        "repo.open",
        &mut s,
        json!({ "path": clone.to_str().unwrap() }),
    )
    .unwrap();
    let config = std::fs::read_to_string(clone.join(".git/config")).unwrap();
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "origin", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("promisor"), "{err}");
    assert_eq!(
        std::fs::read_to_string(clone.join(".git/config")).unwrap(),
        config
    );
}

#[test]
fn errors_of_remote_add_do_not_show_credentials() {
    let Setup { remote: _r, work } = setup();
    // Another git is writing the configuration: `git remote add` fails.
    std::fs::write(work.path().join(".git/config.lock"), "").unwrap();
    let (r, mut s) = open(&work);
    let err = r
        .run(
            "remote.add",
            &mut s,
            json!({ "name": "tok", "url": "https://nicola:secret@example.com/r.git" }),
        )
        .unwrap_err();
    let text = err.to_string();
    assert!(!text.contains("secret"), "{text}");
    assert!(matches!(err, EngineError::Git { .. }), "{err}");
    std::fs::remove_file(work.path().join(".git/config.lock")).unwrap();
}

#[test]
fn keys_without_a_value_survive_undo_and_redo() {
    let Setup { remote: _r, work } = setup();
    let config = work.path().join(".git/config");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("[remote \"origin\"]\n\tskipFetchAll\n");
    std::fs::write(&config, text).unwrap();
    let (r, mut s) = open(&work);
    run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(
        work.git(&["config", "--bool", "remote.origin.skipFetchAll"]),
        "true"
    );
    run(&r, &mut s, "edit.redo", json!({}));
    assert!(work.git(&["remote"]).is_empty());
}

#[test]
fn remove_refuses_a_remote_with_legacy_files() {
    for dir in ["remotes", "branches"] {
        let Setup { remote: _r, work } = setup();
        // Next to the configuration, a file of the old format with the same name.
        std::fs::create_dir_all(work.path().join(".git").join(dir)).unwrap();
        std::fs::write(
            work.path().join(".git").join(dir).join("origin"),
            "URL: /srv/old.git\n",
        )
        .unwrap();
        let (r, mut s) = open(&work);
        let before = state(&work);
        let err = r
            .run(
                "remote.remove",
                &mut s,
                json!({ "name": "origin", "confirm": true }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{dir}: {err}");
        assert!(err.to_string().contains("old format"), "{dir}: {err}");
        assert_eq!(state(&work), before);
    }
}
