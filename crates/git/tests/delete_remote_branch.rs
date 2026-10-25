//! Integration tests of `remote.delete_branch` with the `CliBackend` and a local remote (`git
//! init --bare`) on which a second clone publishes at the same time: confirmation with the commit
//! seen, backup, local remote-tracking branch removed, lease that fails because the branch moved
//! or is already gone, names that start with `-`, `pre-push` hook that rejects, refusals before
//! reaching git.

mod common;

use common::Repo;
use project_domain::Oid;
use project_engine::{EngineError, PushRejection, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

const ID: &str = "remote.delete_branch";

fn open(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

/// The bare remote, a clone that publishes to it (`other`) and the repo under test (`work`).
/// On the remote `main` on `c1` and `feature` on `f1` (a commit only there); `work` fetched both.
struct Setup {
    remote: Repo,
    other: Repo,
    work: Repo,
    f1: Oid,
}

fn setup() -> Setup {
    let remote = Repo::bare();
    let other = Repo::new();
    other.add_remote("origin", &remote);
    other.commit_file("a.txt", "one\n", "first");
    other.push("origin", "HEAD", "main");
    other.branch("feature");
    other.checkout("feature");
    let f1 = other.commit_file("f.txt", "f\n", "feature work");
    other.push("origin", "HEAD", "feature");
    other.checkout("main");
    let work = Repo::new();
    work.add_remote("origin", &remote);
    work.git(&["fetch", "--quiet", "origin"]);
    work.git(&["reset", "--quiet", "--hard", "origin/main"]);
    work.set_upstream("main", "origin");
    Setup {
        remote,
        other,
        work,
        f1,
    }
}

fn on_remote(remote: &Repo, branch: &str) -> Option<Oid> {
    let out = remote.git_output(&[
        "rev-parse",
        "--quiet",
        "--verify",
        &format!("refs/heads/{branch}"),
    ]);
    out.status.success().then(|| {
        String::from_utf8(out.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    })
}

fn all_refs(repo: &Repo) -> String {
    repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"])
}

/// The confirmation request: the reason and the id to send back.
fn confirmation(r: &Registry, s: &mut Session, params: Value) -> (String, String) {
    match r.run(ID, s, params).unwrap_err() {
        EngineError::ConfirmRequired {
            reason,
            expected: Some(expected),
            ..
        } => (reason.to_string(), expected),
        e => panic!("{e}"),
    }
}

#[test]
fn deletes_after_the_confirmation_and_keeps_a_backup() {
    let Setup {
        remote, work, f1, ..
    } = setup();
    work.git(&[
        "branch",
        "--quiet",
        "--track",
        "local-feature",
        "origin/main",
    ]);
    work.git(&[
        "branch",
        "--quiet",
        "--set-upstream-to=origin/feature",
        "local-feature",
    ]);
    let (r, mut s) = open(&work);

    let before = all_refs(&work);
    let params = json!({ "remote": "origin", "branch": "feature" });
    let (reason, expected) = confirmation(&r, &mut s, params);
    assert_eq!(expected, f1.to_string());
    assert!(reason.contains("“feature work”"), "{reason}");
    assert!(reason.contains("`local-feature`"), "{reason}");
    assert!(
        reason.contains(&format!("\"expected\": \"{f1}\"")),
        "{reason}"
    );
    assert_eq!(all_refs(&work), before, "nothing changes without confirm");
    assert_eq!(on_remote(&remote, "feature"), Some(f1));

    let out = r
        .run(
            ID,
            &mut s,
            json!({ "remote": "origin", "branch": "feature", "confirm": true, "expected": expected }),
        )
        .unwrap();
    assert_eq!(out["summary"], "[deleted]");
    assert_eq!(out["deleted"], f1.to_string());
    assert_eq!(out["only_here"], 1);
    assert_eq!(out["lost"][0]["id"], f1.to_string());
    assert_eq!(out["upstream_of"], json!(["local-feature"]));
    assert_eq!(
        out["updated"],
        json!([{ "name": "refs/remotes/origin/feature", "before": f1.to_string(), "after": null }])
    );
    assert_eq!(on_remote(&remote, "feature"), None);
    let backup = out["backup"].as_str().unwrap();
    assert!(
        backup.starts_with("refs/project/backup/push-delete-"),
        "{backup}"
    );
    assert_eq!(work.git(&["rev-parse", backup]), f1.to_string());
    // The upstream stays configured, as git does.
    assert_eq!(
        work.git(&["config", "branch.local-feature.merge"]),
        "refs/heads/feature"
    );
    // Not in the journal.
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["can_undo"], false);
}

#[test]
fn the_lease_fails_if_the_branch_moved_or_is_gone() {
    let Setup {
        remote,
        other,
        work,
        f1,
    } = setup();
    let (r, mut s) = open(&work);
    // Someone else publishes on `feature` after our fetch: the confirmed id is no longer the
    // remote's, git rejects and nothing changes here (the backup is removed).
    other.checkout("feature");
    let f2 = other.commit_file("g.txt", "g\n", "more work");
    other.push("origin", "HEAD", "feature");
    let before = all_refs(&work);
    let err = r
        .run(
            ID,
            &mut s,
            json!({ "remote": "origin", "branch": "feature", "confirm": true, "expected": f1.to_string() }),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::PushRejected {
                reason: PushRejection::StaleLease,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(on_remote(&remote, "feature"), Some(f2));
    assert_eq!(all_refs(&work), before);

    // After the fetch the local remote-tracking branch is f2: the old confirmation is asked again.
    work.git(&["fetch", "--quiet", "origin"]);
    let (reason, expected) = confirmation(
        &r,
        &mut s,
        json!({ "remote": "origin", "branch": "feature", "confirm": true, "expected": f1.to_string() }),
    );
    assert_eq!(expected, f2.to_string());
    assert!(reason.contains("moved"), "{reason}");

    // Deleted by someone else in the meantime: rejected too.
    other.git(&["push", "--quiet", "origin", ":refs/heads/feature"]);
    let before = all_refs(&work);
    let err = r
        .run(
            ID,
            &mut s,
            json!({ "remote": "origin", "branch": "feature", "confirm": true, "expected": expected }),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::PushRejected {
                reason: PushRejection::StaleLease,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(all_refs(&work), before);
}

#[test]
fn names_starting_with_a_dash() {
    let Setup { remote, work, .. } = setup();
    work.git(&["remote", "add", "--", "-r", remote.path_str()]);
    let c2 = work.commit_file("b.txt", "b\n", "second");
    work.git(&["push", "--quiet", "--", "-r", "HEAD:refs/heads/-b"]);
    work.git(&["fetch", "--quiet", "--", "-r"]);
    let (r, mut s) = open(&work);
    let (_, expected) = confirmation(&r, &mut s, json!({ "remote": "-r", "branch": "-b" }));
    assert_eq!(expected, c2.to_string());
    let out = r
        .run(
            ID,
            &mut s,
            json!({ "remote": "-r", "branch": "-b", "confirm": true, "expected": expected }),
        )
        .unwrap();
    assert_eq!(out["summary"], "[deleted]");
    assert_eq!(on_remote(&remote, "-b"), None);
    assert!(on_remote(&remote, "main").is_some());
}

#[test]
fn a_failing_pre_push_hook_stops_the_deletion() {
    let Setup {
        remote, work, f1, ..
    } = setup();
    let hook = work.path().join(".git/hooks/pre-push");
    std::fs::write(&hook, "#!/bin/sh\necho 'pre-push: keep it' >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (r, mut s) = open(&work);
    let before = all_refs(&work);
    let err = r
        .run(
            ID,
            &mut s,
            json!({ "remote": "origin", "branch": "feature", "confirm": true, "expected": f1.to_string() }),
        )
        .unwrap_err();
    let EngineError::PushRejected {
        reason, message, ..
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(*reason, PushRejection::Hook);
    assert!(message.contains("keep it"), "{message}");
    assert_eq!(on_remote(&remote, "feature"), Some(f1));
    assert_eq!(all_refs(&work), before, "the backup is removed too");
}

#[test]
fn rejections_before_reaching_git() {
    let Setup { remote, work, .. } = setup();
    let (r, mut s) = open(&work);
    // Not fetched here: the commit to confirm is not known.
    let other = Repo::new();
    other.add_remote("origin", &remote);
    other.commit_file("x.txt", "x\n", "x");
    other.push("origin", "HEAD", "unseen");
    for params in [
        json!({ "remote": "origin", "branch": "unseen" }),
        json!({ "remote": "missing", "branch": "feature" }),
        json!({ "remote": "--receive-pack=x", "branch": "feature" }),
    ] {
        let err = r.run(ID, &mut s, params.clone()).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{params}: {err}");
    }
    assert!(on_remote(&remote, "unseen").is_some());
    for params in [
        json!({ "remote": "origin" }),
        json!({ "branch": "feature" }),
        json!({ "remote": "origin", "branch": "feature", "confirm": true }),
        json!({ "remote": "origin", "branch": "feature", "confirm": true, "expected": "12ab" }),
        json!({ "remote": "origin", "branch": "refs/heads/feature" }),
        json!({ "remote": "origin", "branch": "feature", "force": true }),
    ] {
        let err = r.run(ID, &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
    // A mirror remote: refused.
    work.git(&["config", "remote.origin.mirror", "true"]);
    let err = r
        .run(
            ID,
            &mut s,
            json!({ "remote": "origin", "branch": "feature" }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("mirror"), "{err}");
}

#[test]
fn several_push_urls_deleted_only_on_some_keeps_the_backup() {
    let Setup {
        remote, work, f1, ..
    } = setup();
    // The second push URL does not have `feature`: there the lease rejects the deletion.
    let mirror = Repo::bare();
    work.git(&[
        "config",
        "--add",
        "remote.origin.pushurl",
        remote.path_str(),
    ]);
    work.git(&[
        "config",
        "--add",
        "remote.origin.pushurl",
        mirror.path_str(),
    ]);
    let (r, mut s) = open(&work);
    let out = r
        .run(
            ID,
            &mut s,
            json!({ "remote": "origin", "branch": "feature", "confirm": true, "expected": f1.to_string() }),
        )
        .unwrap();
    assert_eq!(out["incomplete"], true);
    assert_eq!(on_remote(&remote, "feature"), None);
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(work.git(&["rev-parse", backup]), f1.to_string());
    let warning = out["warnings"][0].to_string();
    assert!(warning.contains("only some"), "{warning}");
}
