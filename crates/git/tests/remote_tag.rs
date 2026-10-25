//! Integration tests of `remote.push_tag` and `remote.delete_tag` with the `CliBackend` and a local
//! remote (`git init --bare`) on which a second clone publishes at the same time: lightweight and
//! annotated tags, a tag already on the remote with the same or another value, names that start with
//! `-`, the confirmation with the value seen, a lease that fails because the tag moved, an object
//! missing locally fetched straight into the backup, `pre-push` hook that rejects.

mod common;

use common::Repo;
use project_domain::Oid;
use project_engine::{
    EngineError, GitBackend, JobContext, PushRejection, Registry, RemoteTagDelete, Session,
};
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

/// The bare remote, a clone that publishes to it (`other`) and the repo under test (`work`), both
/// with `main` on `c1`.
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
    let work = Repo::new();
    work.add_remote("origin", &remote);
    work.git(&["fetch", "--quiet", "origin"]);
    work.git(&["reset", "--quiet", "--hard", "origin/main"]);
    Setup {
        remote,
        other,
        work,
        c1,
    }
}

/// The value of `refs/tags/<name>` on the bare remote (the tag object of an annotated tag).
fn on_remote(remote: &Repo, name: &str) -> Option<Oid> {
    let out = remote.git_output(&[
        "rev-parse",
        "--quiet",
        "--verify",
        "--end-of-options",
        &format!("refs/tags/{name}"),
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

fn backups(repo: &Repo) -> Vec<String> {
    repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/project/backup/",
    ])
    .lines()
    .map(str::to_owned)
    .collect()
}

/// The confirmation request of `remote.delete_tag`: the reason and the id to send back.
fn confirmation(r: &Registry, s: &mut Session, params: Value) -> (String, String) {
    match r.run("remote.delete_tag", s, params).unwrap_err() {
        EngineError::ConfirmRequired {
            reason,
            expected: Some(expected),
            ..
        } => (reason.to_string(), expected),
        e => panic!("{e}"),
    }
}

fn hook(repo: &Repo, script: &str) {
    let hook = repo.path().join(".git/hooks/pre-push");
    std::fs::write(&hook, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn pushes_lightweight_and_annotated_tags_only() {
    let Setup {
        remote, work, c1, ..
    } = setup();
    work.tag("light", "HEAD");
    let object = work.annotated_tag("v1", "HEAD", "Version 1");
    work.tag("not-pushed", "HEAD");
    let c2 = work.commit_file("a.txt", "two\n", "second, local only");
    let (r, mut s) = open(&work);
    let before = all_refs(&work);

    let out = run(
        &r,
        &mut s,
        "remote.push_tag",
        json!({ "remote": "origin", "name": "light" }),
    );
    assert_eq!(out["kind"], "new");
    assert_eq!(out["annotated"], false);
    assert_eq!(out["value"], c1.to_string());
    assert_eq!(on_remote(&remote, "light"), Some(c1));
    let out = run(
        &r,
        &mut s,
        "remote.push_tag",
        json!({ "remote": "origin", "name": "v1" }),
    );
    assert_eq!(out["kind"], "new");
    assert_eq!(out["annotated"], true);
    assert_eq!(out["value"], object.to_string());
    assert_eq!(on_remote(&remote, "v1"), Some(object));
    assert_eq!(
        remote.git(&["cat-file", "-t", "refs/tags/v1"]).trim(),
        "tag"
    );
    // Only the tags: not the other tag, not `main` (ahead locally).
    assert_eq!(on_remote(&remote, "not-pushed"), None);
    assert_eq!(remote.rev("refs/heads/main"), c1);
    assert_ne!(c1, c2);

    // Again: already there with the same value.
    let out = run(
        &r,
        &mut s,
        "remote.push_tag",
        json!({ "remote": "origin", "name": "v1" }),
    );
    assert_eq!(out["kind"], "up_to_date");
    assert_eq!(all_refs(&work), before, "no local ref changes");
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["can_undo"], false);
}

#[test]
fn a_tag_already_on_the_remote_with_another_value_is_never_overwritten() {
    let Setup {
        remote,
        other,
        work,
        ..
    } = setup();
    let c2 = other.commit_file("a.txt", "two\n", "second");
    other.push("origin", "HEAD", "main");
    other.tag("v2", "HEAD");
    other.git(&["push", "--quiet", "origin", "refs/tags/v2:refs/tags/v2"]);
    work.annotated_tag("v2", "HEAD", "mine");
    let (r, mut s) = open(&work);
    let err = r
        .run(
            "remote.push_tag",
            &mut s,
            json!({ "remote": "origin", "name": "v2" }),
        )
        .unwrap_err();
    let EngineError::PushRejected {
        reason,
        branch,
        message,
        ..
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(*reason, PushRejection::TagExists);
    assert_eq!(branch, "refs/tags/v2");
    assert!(message.contains("already exists"), "{message}");
    assert_eq!(on_remote(&remote, "v2"), Some(c2));
    // Even with `push.followTags` and a `--force`-like configuration nothing else goes.
    work.git(&["config", "push.followTags", "true"]);
    work.git(&["config", "remote.origin.push", "+refs/tags/*:refs/tags/*"]);
    let err = r
        .run(
            "remote.push_tag",
            &mut s,
            json!({ "remote": "origin", "name": "v2" }),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::PushRejected {
                reason: PushRejection::TagExists,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(on_remote(&remote, "v2"), Some(c2));
}

#[test]
fn names_starting_with_a_dash() {
    let Setup {
        remote, work, c1, ..
    } = setup();
    for name in ["-x", "--force"] {
        work.git(&["update-ref", &format!("refs/tags/{name}"), &c1.to_string()]);
    }
    let (r, mut s) = open(&work);
    for name in ["-x", "--force"] {
        let out = run(
            &r,
            &mut s,
            "remote.push_tag",
            json!({ "remote": "origin", "name": name }),
        );
        assert_eq!(out["kind"], "new", "{name}");
        assert_eq!(on_remote(&remote, name), Some(c1), "{name}");
    }
    for name in ["-x", "--force"] {
        let (_, expected) = confirmation(&r, &mut s, json!({ "remote": "origin", "name": name }));
        assert_eq!(expected, c1.to_string());
        let out = run(
            &r,
            &mut s,
            "remote.delete_tag",
            json!({ "remote": "origin", "name": name, "confirm": true, "expected": expected }),
        );
        assert_eq!(out["tag"], format!("refs/tags/{name}"));
        assert_eq!(on_remote(&remote, name), None, "{name}");
    }
    // The local tags stay.
    assert_eq!(work.rev("refs/tags/-x"), c1);
    assert_eq!(work.rev("refs/tags/--force"), c1);
}

#[test]
fn deletes_an_annotated_tag_after_the_confirmation_and_keeps_a_backup() {
    let Setup {
        remote, work, c1, ..
    } = setup();
    let object = work.annotated_tag("v1", "HEAD", "Version 1");
    work.git(&["push", "--quiet", "origin", "refs/tags/v1:refs/tags/v1"]);
    let (r, mut s) = open(&work);
    let before = all_refs(&work);
    let (reason, expected) = confirmation(&r, &mut s, json!({ "remote": "origin", "name": "v1" }));
    // The value as stored (the tag object), not the peeled commit.
    assert_eq!(expected, object.to_string());
    assert_ne!(expected, c1.to_string());
    assert!(reason.contains("same value"), "{reason}");
    assert!(
        reason.contains(&format!("\"expected\": \"{object}\"")),
        "{reason}"
    );
    assert_eq!(all_refs(&work), before, "nothing changes without confirm");
    assert_eq!(on_remote(&remote, "v1"), Some(object));

    let out = run(
        &r,
        &mut s,
        "remote.delete_tag",
        json!({ "remote": "origin", "name": "v1", "confirm": true, "expected": expected }),
    );
    assert_eq!(out["summary"], "[deleted]");
    assert_eq!(out["deleted"], object.to_string());
    assert_eq!(out["incomplete"], false);
    assert_eq!(on_remote(&remote, "v1"), None);
    let backup = out["backup"].as_str().unwrap();
    assert!(backup.starts_with("refs/project/backup/push-delete-"));
    assert_eq!(work.rev(backup), object);
    // The local tag is not touched; nothing in the journal.
    assert_eq!(work.rev("refs/tags/v1"), object);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["can_undo"], false);

    // Gone now: refused before the push.
    let err = r
        .run(
            "remote.delete_tag",
            &mut s,
            json!({ "remote": "origin", "name": "v1", "confirm": true, "expected": object.to_string() }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
}

#[test]
fn the_lease_fails_if_the_tag_moved_after_it_was_seen() {
    let Setup {
        remote,
        other,
        work,
        c1,
    } = setup();
    work.tag("t", "HEAD");
    work.git(&["push", "--quiet", "origin", "refs/tags/t:refs/tags/t"]);
    let (r, mut s) = open(&work);
    let (_, seen) = confirmation(&r, &mut s, json!({ "remote": "origin", "name": "t" }));
    assert_eq!(seen, c1.to_string());

    // Another clone moves the tag on the remote.
    let c2 = other.commit_file("a.txt", "two\n", "second");
    other.push("origin", "HEAD", "main");
    other.git(&["tag", "-f", "t", "HEAD"]);
    other.git(&[
        "push",
        "--quiet",
        "--force",
        "origin",
        "refs/tags/t:refs/tags/t",
    ]);

    // The command reads the remote again: the confirmation is asked again with the new value.
    let err = r
        .run(
            "remote.delete_tag",
            &mut s,
            json!({ "remote": "origin", "name": "t", "confirm": true, "expected": seen }),
        )
        .unwrap_err();
    let EngineError::ConfirmRequired {
        expected, reason, ..
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(expected.as_deref(), Some(c2.to_string().as_str()));
    assert!(reason.contains("another value"), "{reason}");
    assert_eq!(on_remote(&remote, "t"), Some(c2));

    // Moved between the read and the push (the backend with the old value): the lease rejects it
    // and the backup is removed.
    let before = all_refs(&work);
    let err = work
        .open()
        .delete_remote_tag(
            &RemoteTagDelete {
                remote: "origin".into(),
                name: "t".into(),
                expected: c1,
            },
            &JobContext::new(),
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
    assert_eq!(on_remote(&remote, "t"), Some(c2));
    assert_eq!(all_refs(&work), before);
}

#[test]
fn an_object_missing_here_is_fetched_into_the_backup_only() {
    let Setup {
        remote,
        other,
        work,
        ..
    } = setup();
    let c2 = other.commit_file("a.txt", "two\n", "second, never fetched here");
    let object = other.annotated_tag("ro", "HEAD", "Remote only");
    other.git(&["push", "--quiet", "origin", "refs/tags/ro:refs/tags/ro"]);
    // A refspec that would also write the tags here: `--refmap=` keeps it out.
    work.git(&[
        "config",
        "--add",
        "remote.origin.fetch",
        "+refs/tags/*:refs/tags/*",
    ]);
    work.git(&["config", "remote.origin.tagOpt", "--tags"]);
    let fetch_head = work.path().join(".git/FETCH_HEAD");
    let _ = std::fs::remove_file(&fetch_head);
    let (r, mut s) = open(&work);
    let (reason, expected) = confirmation(&r, &mut s, json!({ "remote": "origin", "name": "ro" }));
    assert_eq!(expected, object.to_string());
    assert!(reason.contains("no local tag"), "{reason}");
    assert!(
        !work
            .git_output(&["cat-file", "-e", &expected])
            .status
            .success()
    );

    let out = run(
        &r,
        &mut s,
        "remote.delete_tag",
        json!({ "remote": "origin", "name": "ro", "confirm": true, "expected": expected }),
    );
    let backup = out["backup"].as_str().unwrap();
    assert!(backup.starts_with("refs/project/backup/push-delete-"));
    assert_eq!(work.rev(backup), object);
    assert_eq!(work.git(&["cat-file", "-t", backup]).trim(), "tag");
    assert_eq!(work.rev(&format!("{backup}^{{commit}}")), c2);
    assert_eq!(on_remote(&remote, "ro"), None);
    // Only the backup was written: no local tag, no remote branch moved, no FETCH_HEAD.
    assert!(work.git(&["tag", "--list"]).trim().is_empty());
    assert_ne!(work.rev("refs/remotes/origin/main"), c2);
    assert!(!fetch_head.exists());
}

#[test]
fn a_missing_object_that_moved_on_the_remote_leaves_nothing() {
    let Setup {
        remote,
        other,
        work,
        ..
    } = setup();
    other.commit_file("a.txt", "two\n", "second");
    let old = other.annotated_tag("ro", "HEAD", "first version");
    other.git(&["push", "--quiet", "origin", "refs/tags/ro:refs/tags/ro"]);
    let newer = other.commit_file("a.txt", "three\n", "third");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    other.git(&["tag", "-f", "ro", "HEAD"]);
    other.git(&[
        "push",
        "--quiet",
        "--force",
        "origin",
        "refs/tags/ro:refs/tags/ro",
    ]);
    let before = all_refs(&work);
    let err = work
        .open()
        .delete_remote_tag(
            &RemoteTagDelete {
                remote: "origin".into(),
                name: "ro".into(),
                expected: old,
            },
            &JobContext::new(),
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
    assert_eq!(on_remote(&remote, "ro"), Some(newer));
    assert_eq!(all_refs(&work), before);
    assert!(backups(&work).is_empty());
}

#[test]
fn a_failing_pre_push_hook_stops_push_and_deletion() {
    let Setup {
        remote, work, c1, ..
    } = setup();
    work.tag("t", "HEAD");
    work.tag("new", "HEAD");
    work.git(&["push", "--quiet", "origin", "refs/tags/t:refs/tags/t"]);
    hook(&work, "#!/bin/sh\necho 'pre-push: keep it' >&2\nexit 1\n");
    let (r, mut s) = open(&work);
    let before = all_refs(&work);
    let err = r
        .run(
            "remote.push_tag",
            &mut s,
            json!({ "remote": "origin", "name": "new" }),
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
    assert_eq!(on_remote(&remote, "new"), None);

    let err = r
        .run(
            "remote.delete_tag",
            &mut s,
            json!({ "remote": "origin", "name": "t", "confirm": true, "expected": c1.to_string() }),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::PushRejected {
                reason: PushRejection::Hook,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(on_remote(&remote, "t"), Some(c1));
    assert_eq!(all_refs(&work), before, "the backup is removed too");
}

#[test]
fn rejections_before_reaching_git() {
    let Setup {
        work,
        remote: _remote,
        ..
    } = setup();
    work.tag("t", "HEAD");
    work.git(&["remote", "add", "--mirror=push", "mirror", "/nonexistent"]);
    let (r, mut s) = open(&work);
    let before = all_refs(&work);
    for (id, params) in [
        (
            "remote.push_tag",
            json!({ "remote": "origin", "name": "nothing" }),
        ),
        (
            "remote.push_tag",
            json!({ "remote": "nowhere", "name": "t" }),
        ),
        (
            "remote.push_tag",
            json!({ "remote": "mirror", "name": "t" }),
        ),
        (
            "remote.delete_tag",
            json!({ "remote": "origin", "name": "t" }),
        ),
        (
            "remote.delete_tag",
            json!({ "remote": "mirror", "name": "t" }),
        ),
    ] {
        let err = r.run(id, &mut s, params.clone()).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{params}: {err}");
    }
    let err = r
        .run(
            "remote.delete_tag",
            &mut s,
            json!({ "remote": "origin", "name": "t", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    assert_eq!(all_refs(&work), before);
}

#[test]
fn unreachable_remote() {
    let Setup {
        work,
        remote: _remote,
        ..
    } = setup();
    work.tag("t", "HEAD");
    work.git(&["remote", "add", "gone", "/nonexistent/repo.git"]);
    let (r, mut s) = open(&work);
    let err = r
        .run(
            "remote.push_tag",
            &mut s,
            json!({ "remote": "gone", "name": "t" }),
        )
        .unwrap_err();
    assert!(
        matches!(err, EngineError::RemoteUnreachable { .. }),
        "{err}"
    );
    let err = r
        .run(
            "remote.delete_tag",
            &mut s,
            json!({ "remote": "gone", "name": "t" }),
        )
        .unwrap_err();
    assert!(
        matches!(err, EngineError::RemoteUnreachable { .. }),
        "{err}"
    );
}
