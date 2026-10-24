//! Integration tests of `remote.push` (S26) with the `CliBackend` and a local remote (`git
//! init --bare`) on which a second clone publishes at the same time: first push with
//! the upstream, normal push, non-fast-forward rejection, force without confirmation,
//! force-with-lease that succeeds and that fails because the remote moved, `push.default=matching`
//! and `push.followTags`, names that start with `-`, `pre-push` hook that rejects.

mod common;

use common::Repo;
use project_domain::Oid;
use project_engine::{EngineError, PushRejection, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn open(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

fn push(r: &Registry, s: &mut Session, params: Value) -> Value {
    r.run("remote.push", s, params)
        .unwrap_or_else(|e| panic!("remote.push: {e}"))
}

/// The bare remote, a clone that publishes to it (`other`) and the repo under test (`work`), with
/// `main` on `c1` in all three; `main` follows `origin/main`.
struct Setup {
    remote: Repo,
    other: Repo,
    work: Repo,
    c1: Oid,
}

fn setup() -> Setup {
    let remote = Repo::bare();
    let other = Repo::new();
    other.add_remote("origin", &remote);
    let c1 = other.commit_file("a.txt", "one\n", "first");
    other.push("origin", "HEAD", "main");
    other.set_upstream("main", "origin");
    let work = Repo::new();
    work.add_remote("origin", &remote);
    work.git(&["fetch", "--quiet", "origin"]);
    work.git(&["reset", "--quiet", "--hard", "origin/main"]);
    work.set_upstream("main", "origin");
    Setup {
        remote,
        other,
        work,
        c1,
    }
}

/// The commit of the branch `branch` on the remote, if any.
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

#[test]
fn first_push_sets_the_upstream() {
    let Setup {
        remote, work, c1, ..
    } = setup();
    work.branch("new-branch");
    work.checkout("new-branch");
    let n1 = work.commit_file("n.txt", "n\n", "new-branch");
    let (r, mut s) = open(&work);

    let out = push(&r, &mut s, json!({}));
    assert_eq!(out["remote"], "origin");
    assert_eq!(out["branch"], "new-branch");
    assert_eq!(out["remote_branch"], "new-branch");
    assert_eq!(out["kind"], "new");
    assert_eq!(out["summary"], "[new branch]");
    assert_eq!(out["set_upstream"], true);
    assert_eq!(out["forced"], false);
    assert_eq!(out["before"], Value::Null);
    assert_eq!(out["after"], n1.to_string());
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(
        out["updated"],
        json!([{ "name": "refs/remotes/origin/new-branch", "before": null, "after": n1.to_string() }])
    );
    assert!(
        out["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m.as_str().unwrap().starts_with("To ")),
        "{}",
        out["messages"]
    );
    assert_eq!(on_remote(&remote, "new-branch"), Some(n1));
    assert_eq!(on_remote(&remote, "main"), Some(c1));
    assert_eq!(
        work.git(&["rev-parse", "--abbrev-ref", "new-branch@{upstream}"]),
        "origin/new-branch"
    );
    assert_eq!(out["status"]["upstream"]["name"], "origin/new-branch");
    // It does not go in the journal.
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["can_undo"], false);

    // With several remotes and no `origin` the first push asks which one.
    work.git(&["remote", "rename", "origin", "a"]);
    work.git(&["remote", "add", "b", remote.path_str()]);
    work.branch("other");
    let err = r
        .run("remote.push", &mut s, json!({ "branch": "other" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let out = push(&r, &mut s, json!({ "branch": "other", "remote": "b" }));
    assert_eq!(out["set_upstream"], true);
    assert_eq!(
        work.git(&["rev-parse", "--abbrev-ref", "other@{upstream}"]),
        "b/other"
    );
}

#[test]
fn push_fast_forward_and_up_to_date() {
    let Setup {
        remote, work, c1, ..
    } = setup();
    let c2 = work.commit_file("b.txt", "b\n", "second");
    let (r, mut s) = open(&work);
    let out = push(&r, &mut s, json!({}));
    assert_eq!(out["kind"], "fast_forward");
    assert_eq!(out["set_upstream"], false);
    assert_eq!(out["before"], c1.to_string());
    assert_eq!(out["after"], c2.to_string());
    assert_eq!(
        out["summary"],
        format!("{}..{}", &c1.to_string()[..7], &c2.to_string()[..7])
    );
    assert_eq!(
        out["updated"],
        json!([{ "name": "refs/remotes/origin/main", "before": c1.to_string(), "after": c2.to_string() }])
    );
    assert_eq!(on_remote(&remote, "main"), Some(c2));
    assert_eq!(out["status"]["upstream"]["ahead"], 0);

    let out = push(&r, &mut s, json!({ "remote": "origin", "branch": "main" }));
    assert_eq!(out["kind"], "up_to_date");
    assert_eq!(out["updated"], json!([]));
}

#[test]
fn non_fast_forward_is_rejected_and_nothing_changes() {
    let Setup {
        remote,
        other,
        work,
        c1,
    } = setup();
    let theirs = other.commit_file("b.txt", "theirs\n", "theirs");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    work.commit_file("c.txt", "mine\n", "mine");
    let (r, mut s) = open(&work);
    let before = all_refs(&work);

    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    let EngineError::PushRejected {
        remote: name,
        branch,
        reason,
        message,
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!((name.as_str(), branch.as_str()), ("origin", "main"));
    assert_eq!(*reason, PushRejection::NonFastForward);
    assert!(message.contains("[rejected]"), "{message}");
    assert!(err.to_string().contains("remote.pull"), "{err}");
    assert_eq!(on_remote(&remote, "main"), Some(theirs));
    assert_eq!(all_refs(&work), before);
    assert_eq!(work.rev("origin/main"), c1);

    // Also when the local remote branch already knows the commit (after a fetch).
    work.git(&["fetch", "--quiet", "origin"]);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::PushRejected {
                reason: PushRejection::NonFastForward,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn force_needs_confirm_and_uses_the_lease() {
    let Setup {
        remote,
        other,
        work,
        c1,
    } = setup();
    let theirs = other.commit_file("b.txt", "theirs\n", "commit from others");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    let mine = work.commit_file("c.txt", "mine\n", "mine");
    work.git(&["fetch", "--quiet", "origin"]);
    let (r, mut s) = open(&work);
    let before = all_refs(&work);

    // Without confirmation: the error lists the commits the remote would lose, nothing changes.
    for params in [
        json!({ "force": true }),
        json!({ "force": true, "confirm": false }),
    ] {
        let err = r.run("remote.push", &mut s, params).unwrap_err();
        let EngineError::ConfirmRequired {
            command,
            reason,
            expected,
        } = &err
        else {
            panic!("{err}");
        };
        assert_eq!(*command, "remote.push");
        assert_eq!(expected.as_deref(), Some(theirs.to_string().as_str()));
        assert!(reason.contains("commit from others"), "{reason}");
        assert!(reason.contains(&theirs.to_string()[..7]), "{reason}");
        assert!(reason.contains("1 commit"), "{reason}");
        // The id to pass in `expected`, in a fixed form.
        assert!(
            reason.contains(&format!("\"expected\": \"{theirs}\"")),
            "{reason}"
        );
    }
    for params in [
        json!({ "confirm": true }),
        json!({ "force": true, "confirm": true }),
        json!({ "force": true, "confirm": true, "expected": "abc" }),
        json!({ "expected": theirs.to_string() }),
    ] {
        let err = r.run("remote.push", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
    assert_eq!(on_remote(&remote, "main"), Some(theirs));
    assert_eq!(all_refs(&work), before);

    // With confirmation: force-with-lease, and first the backup of the overwritten commit.
    let out = push(
        &r,
        &mut s,
        json!({ "force": true, "confirm": true, "expected": theirs.to_string() }),
    );
    assert_eq!(out["kind"], "forced");
    assert_eq!(out["forced"], true);
    assert_eq!(out["before"], theirs.to_string());
    assert_eq!(out["after"], mine.to_string());
    assert_eq!(out["lost"][0]["id"], theirs.to_string());
    let backup = out["backup"].as_str().unwrap();
    assert!(backup.starts_with("refs/project/backup/push-"), "{backup}");
    assert_eq!(work.rev(backup), theirs);
    assert_eq!(on_remote(&remote, "main"), Some(mine));
    assert_eq!(work.rev("origin/main"), mine);
    let _ = c1;

    // Force with no commits to lose: no confirmation, no backup.
    let more = work.commit_file("d.txt", "d\n", "again");
    let out = push(&r, &mut s, json!({ "force": true }));
    assert_eq!(out["kind"], "fast_forward");
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(on_remote(&remote, "main"), Some(more));
}

#[test]
fn the_lease_fails_if_the_remote_moved() {
    let Setup {
        remote,
        other,
        work,
        ..
    } = setup();
    other.commit_file("b.txt", "theirs\n", "theirs");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    work.commit_file("c.txt", "mine\n", "mine");
    work.git(&["fetch", "--quiet", "origin"]);
    // After the fetch someone publishes again: the local remote branch is old.
    let newer = other.commit_file("b.txt", "theirs 2\n", "theirs 2");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    let (r, mut s) = open(&work);
    let before = all_refs(&work);
    let seen = work.rev("origin/main");

    let err = r
        .run(
            "remote.push",
            &mut s,
            json!({ "force": true, "confirm": true, "expected": seen.to_string() }),
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
    assert!(err.to_string().contains("remote.fetch"), "{err}");
    assert_eq!(on_remote(&remote, "main"), Some(newer));
    // Nothing changes locally, not even the backup.
    assert_eq!(all_refs(&work), before);
}

#[test]
fn only_the_branch_is_pushed_whatever_the_config() {
    let Setup {
        remote, work, c1, ..
    } = setup();
    // `other` also exists on the remote, behind: with `push.default=matching` a `git push`
    // would publish it.
    work.branch("other");
    work.git(&["push", "--quiet", "origin", "other"]);
    work.checkout("other");
    let a2 = work.commit_file("x.txt", "x\n", "on other");
    work.checkout("main");
    let c2 = work.commit_file("b.txt", "b\n", "on main");
    work.git(&["tag", "v1"]);
    work.git(&["config", "push.default", "matching"]);
    work.git(&["config", "push.followTags", "true"]);
    work.git(&["config", "remote.origin.push", "refs/heads/*:refs/heads/*"]);
    let (r, mut s) = open(&work);

    push(&r, &mut s, json!({}));
    assert_eq!(on_remote(&remote, "main"), Some(c2));
    assert_eq!(on_remote(&remote, "other"), Some(c1));
    assert!(remote.git(&["tag"]).is_empty(), "no tag published");
    let _ = a2;
}

#[test]
fn names_starting_with_a_dash() {
    let Setup { remote, work, .. } = setup();
    work.git(&["remote", "add", "--", "-r", remote.path_str()]);
    work.git(&["update-ref", "refs/heads/-b", "HEAD"]);
    work.git(&["update-ref", "refs/heads/--force", "HEAD"]);
    let c2 = work.commit_file("b.txt", "b\n", "second");
    work.git(&["update-ref", "refs/heads/-b", &c2.to_string()]);
    let (r, mut s) = open(&work);

    let out = push(&r, &mut s, json!({ "remote": "-r", "branch": "-b" }));
    assert_eq!(out["kind"], "new");
    assert_eq!(on_remote(&remote, "-b"), Some(c2));
    assert_eq!(
        work.git(&["config", "branch.-b.remote"]),
        "-r",
        "upstream set"
    );
    // `--force` is a branch, not an option: published like the others, without forcing.
    let out = push(&r, &mut s, json!({ "remote": "-r", "branch": "--force" }));
    assert_eq!(out["kind"], "new");
    assert_eq!(out["forced"], false);
    // A remote that does not exist does not reach git.
    let err = r
        .run(
            "remote.push",
            &mut s,
            json!({ "remote": "--receive-pack=x" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    // And the pull of the branch just published: already up to date.
    let out = r
        .run("remote.pull", &mut s, json!({ "branch": "-b" }))
        .unwrap();
    assert_eq!(out["strategy"], "up_to_date");
}

#[test]
fn a_failing_pre_push_hook_stops_the_push() {
    let Setup {
        remote, work, c1, ..
    } = setup();
    work.commit_file("b.txt", "b\n", "second");
    let hook = work.path().join(".git/hooks/pre-push");
    std::fs::write(
        &hook,
        "#!/bin/sh\necho 'pre-push: no push today' >&2\nexit 1\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (r, mut s) = open(&work);
    let before = all_refs(&work);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    let EngineError::PushRejected {
        reason, message, ..
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(*reason, PushRejection::Hook);
    assert!(message.contains("no push today"), "{message}");
    assert_eq!(on_remote(&remote, "main"), Some(c1));
    assert_eq!(all_refs(&work), before);
}

#[test]
fn rejections_before_reaching_git() {
    let Setup { work, c1, .. } = setup();
    let (r, mut s) = open(&work);
    work.detach(c1);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("detached"), "{err}");
    work.checkout("main");
    // An upstream with a different name: rejected as `push.default=simple` does.
    work.branch("feature");
    work.git(&[
        "branch",
        "--quiet",
        "--set-upstream-to=origin/main",
        "feature",
    ]);
    let err = r
        .run("remote.push", &mut s, json!({ "branch": "feature" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("simple"), "{err}");
    for params in [
        json!({ "tags": true }),
        json!({ "branch": "refs/heads/main" }),
        json!({ "remote": "" }),
        json!({ "force": "yes" }),
    ] {
        let err = r.run("remote.push", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
}

#[test]
fn unreachable_remote_and_cancel() {
    let Setup {
        remote, work, c1, ..
    } = setup();
    work.commit_file("b.txt", "b\n", "second");
    let missing = work.path().join("does-not-exist.git");
    work.git(&["remote", "set-url", "origin", missing.to_str().unwrap()]);
    let (r, mut s) = open(&work);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(err, EngineError::RemoteUnreachable { .. }),
        "{err}"
    );
    work.git(&["remote", "set-url", "origin", remote.path_str()]);
    let token = project_engine::CancelToken::new();
    token.cancel();
    let err = r
        .run_with(
            "remote.push",
            &mut s,
            json!({}),
            project_engine::JobContext::new().with_cancel(token),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Cancelled {
                command: "remote.push"
            }
        ),
        "{err}"
    );
    assert_eq!(on_remote(&remote, "main"), Some(c1));
}

#[test]
fn the_confirmation_is_for_the_commit_that_was_seen() {
    let Setup {
        remote,
        other,
        work,
        ..
    } = setup();
    let theirs = other.commit_file("b.txt", "theirs\n", "theirs");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    work.commit_file("c.txt", "mine\n", "mine");
    work.git(&["fetch", "--quiet", "origin"]);
    let (r, mut s) = open(&work);
    let err = r
        .run("remote.push", &mut s, json!({ "force": true }))
        .unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");

    // Between the confirmation and the push a fetch brings one more commit: nothing published,
    // confirmation is asked again with the new id.
    let newer = other.commit_file("b.txt", "theirs 2\n", "theirs 2");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    work.git(&["fetch", "--quiet", "origin"]);
    let before = all_refs(&work);
    let err = r
        .run(
            "remote.push",
            &mut s,
            json!({ "force": true, "confirm": true, "expected": theirs.to_string() }),
        )
        .unwrap_err();
    let EngineError::ConfirmRequired {
        reason, expected, ..
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(expected.as_deref(), Some(newer.to_string().as_str()));
    assert!(reason.contains("moved since it was seen"), "{reason}");
    assert!(reason.contains("2 commits"), "{reason}");
    assert!(
        reason.contains(&format!("\"expected\": \"{newer}\"")),
        "{reason}"
    );
    assert_eq!(on_remote(&remote, "main"), Some(newer));
    assert_eq!(all_refs(&work), before);
}

#[test]
fn push_remote_and_push_default_like_git() {
    // `origin` is the upstream (where it fetches from), `fork` is where it publishes.
    let Setup {
        remote, work, c1, ..
    } = setup();
    let fork = Repo::bare();
    work.add_remote("fork", &fork);
    // A non-standard fetch refspec: the local remote branch is `refs/remotes/mine/*`.
    work.git(&[
        "config",
        "--replace-all",
        "remote.fork.fetch",
        "+refs/heads/*:refs/remotes/mine/*",
    ]);
    let c2 = work.commit_file("b.txt", "b\n", "second");
    work.git(&["config", "remote.pushDefault", "fork"]);
    let (r, mut s) = open(&work);

    let out = push(&r, &mut s, json!({}));
    assert_eq!(out["remote"], "fork");
    assert_eq!(out["remote_source"], "push_default");
    assert_eq!(out["kind"], "new");
    assert_eq!(
        out["set_upstream"], false,
        "the upstream stays `origin/main`"
    );
    assert_eq!(on_remote(&fork, "main"), Some(c2));
    assert_eq!(on_remote(&remote, "main"), Some(c1));
    assert_eq!(
        out["updated"],
        json!([{ "name": "refs/remotes/mine/main", "before": null, "after": c2.to_string() }])
    );
    assert_eq!(work.git(&["config", "branch.main.remote"]), "origin");

    // `branch.<name>.pushRemote` wins over `remote.pushDefault`.
    work.git(&["config", "branch.main.pushRemote", "origin"]);
    let out = push(&r, &mut s, json!({}));
    assert_eq!(out["remote"], "origin");
    assert_eq!(out["remote_source"], "push_remote");
    assert_eq!(on_remote(&remote, "main"), Some(c2));
    // `remote` wins over all.
    let out = push(&r, &mut s, json!({ "remote": "fork" }));
    assert_eq!(out["remote_source"], "param");
    assert_eq!(out["kind"], "up_to_date");

    // A configured remote that does not exist: rejected, nothing published.
    work.git(&["config", "branch.main.pushRemote", "gone"]);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("pushRemote"), "{err}");
    work.git(&["config", "--unset", "branch.main.pushRemote"]);
    work.git(&["config", "--unset", "remote.pushDefault"]);
    let out = push(&r, &mut s, json!({}));
    assert_eq!(out["remote_source"], "upstream");

    // A mirror remote: rejected with an explanation.
    work.git(&["config", "remote.fork.mirror", "true"]);
    let err = r
        .run("remote.push", &mut s, json!({ "remote": "fork" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("mirror"), "{err}");
}
