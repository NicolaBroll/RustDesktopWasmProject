//! Integration tests of `commit.checkout` (S55) with the `CliBackend` on real repos: checkout
//! of an old commit by id, short id, tag and branch name, a rev starting with `-`, refusals
//! (local changes, operation in progress, not a commit), undo/redo, and the backup of a
//! detached HEAD left behind on commits that no branch, tag or remote branch reaches (also by
//! `branch.checkout`), with the backup failing and the switch failing after it.

mod common;

use common::Repo;
use project_domain::Oid;
use project_engine::{EngineError, Registry, Session};
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

fn detached(o: Oid) -> Value {
    json!({ "detached": o.to_string() })
}

/// `None` if HEAD is detached, otherwise its branch.
fn head_branch(repo: &Repo) -> Option<String> {
    let out = repo.git_output(&["symbolic-ref", "-q", "HEAD"]);
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The `refs/project/backup/detached-*` refs with their commits.
fn detached_backups(repo: &Repo) -> Vec<(String, Oid)> {
    repo.git(&[
        "for-each-ref",
        "--format=%(refname) %(objectname)",
        "refs/project/backup/",
    ])
    .lines()
    .filter(|l| l.starts_with("refs/project/backup/detached-"))
    .map(|l| {
        let (name, id) = l.split_once(' ').unwrap();
        (name.to_owned(), id.parse().unwrap())
    })
    .collect()
}

/// `main`: `first` (a.txt = "a") → `second` (a.txt = "b", tag `v1`); HEAD on `main`.
fn history() -> (Repo, Oid, Oid) {
    let repo = Repo::new();
    let first = repo.commit_file("a.txt", "a\n", "first");
    let second = repo.commit_file("a.txt", "b\n", "second");
    repo.tag("v1", "HEAD");
    (repo, first, second)
}

/// [`history`] with a commit made on a detached HEAD at `second` (on no ref); HEAD there.
fn with_loose() -> (Repo, Oid, Oid, Oid) {
    let (repo, first, second) = history();
    repo.detach(second);
    let loose = repo.commit_file("c.txt", "loose\n", "loose");
    (repo, first, second, loose)
}

fn install_hook(repo: &Repo, name: &str, body: &str) {
    let hook = repo.path().join(".git/hooks").join(name);
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, format!("#!/bin/sh\n{body}")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn checks_out_an_old_commit_then_undo_and_redo() {
    let (repo, first, second) = history();
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "commit.checkout",
        json!({ "rev": first.to_string() }),
    );
    assert_eq!(out["head"], detached(first));
    assert_eq!(out["previous"], json!({ "branch": "refs/heads/main" }));
    assert_eq!(out["changed"], true);
    assert_eq!(out["warnings"], json!([]));
    assert!(out.get("backup").is_none(), "{out}");
    assert_eq!(out["status"]["head"]["Detached"], json!(first.to_string()));
    assert_eq!(head_branch(&repo), None);
    assert_eq!(repo.rev("HEAD"), first);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a\n"
    );

    // Already there (by short id): nothing changes, nothing in the journal.
    let short = &first.to_string()[..7];
    let out = run(&r, &mut s, "commit.checkout", json!({ "rev": short }));
    assert_eq!(out["changed"], false);
    assert_eq!(s.journal().unwrap().done().len(), 1);

    // A tag.
    let out = run(&r, &mut s, "commit.checkout", json!({ "rev": "v1" }));
    assert_eq!(out["head"], detached(second));
    assert_eq!(out["previous"], detached(first));

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["undone"]["command"], "commit.checkout");
    assert_eq!(repo.rev("HEAD"), first);
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(head_branch(&repo).as_deref(), Some("refs/heads/main"));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(head_branch(&repo), None);
    assert_eq!(repo.rev("HEAD"), first);

    // "Create a branch here": `branch.create` without `start` from the detached HEAD.
    let out = run(&r, &mut s, "branch.create", json!({ "name": "here" }));
    assert_eq!(out["target"], first.to_string());
    assert_eq!(repo.rev("refs/heads/here"), first);
    assert_eq!(head_branch(&repo), None);

    // Undo refused if HEAD changed from outside.
    repo.checkout("main");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(head_branch(&repo).as_deref(), Some("refs/heads/main"));
    assert!(detached_backups(&repo).is_empty());
}

#[test]
fn names_starting_with_a_dash_and_revs_that_are_not_commits() {
    let (repo, first, _) = history();
    repo.git(&["update-ref", "refs/heads/-x", &first.to_string()]);
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.checkout", json!({ "rev": "-x" }));
    assert_eq!(out["head"], detached(first));
    run(&r, &mut s, "edit.undo", json!({}));

    for rev in ["--force", "-d", "missing", "HEAD~9"] {
        let err = r
            .run("commit.checkout", &mut s, json!({ "rev": rev }))
            .unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::UnknownRev(_) | EngineError::InvalidParams { .. }
            ),
            "{rev}: {err}"
        );
    }
    for rev in ["HEAD^{tree}", "HEAD:a.txt", ""] {
        let err = r
            .run("commit.checkout", &mut s, json!({ "rev": rev }))
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{rev}: {err}"
        );
    }
    assert_eq!(head_branch(&repo).as_deref(), Some("refs/heads/main"));
    assert_eq!(s.journal().unwrap().done().len(), 0);
}

#[test]
fn refused_with_conflicting_changes_or_during_an_operation() {
    let (repo, first, second) = history();
    repo.write("a.txt", "my change\n");
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "commit.checkout",
            &mut s,
            json!({ "rev": first.to_string() }),
        )
        .unwrap_err();
    let EngineError::CheckoutConflict { target, files, .. } = &err else {
        panic!("{err}");
    };
    assert_eq!(target, &first.to_string());
    assert_eq!(files, &["a.txt"]);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "my change\n"
    );
    assert_eq!(repo.rev("HEAD"), second);
    assert!(!s.journal().unwrap().can_undo());

    // Merge stopped on a conflict.
    let (repo, first, _) = history();
    repo.diverge("other", "a.txt", "ours\n", "theirs\n");
    repo.conflict("other");
    let head = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "commit.checkout",
            &mut s,
            json!({ "rev": first.to_string() }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(repo.rev("HEAD"), head);
    assert!(repo.path().join(".git/MERGE_HEAD").exists());
}

#[test]
fn leaving_a_commit_on_no_ref_saves_it_first() {
    let (repo, first, _, loose) = with_loose();
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "commit.checkout",
        json!({ "rev": first.to_string() }),
    );
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(backup.starts_with("refs/project/backup/detached-"), "{out}");
    assert_eq!(detached_backups(&repo), [(backup.clone(), loose)]);
    let warning = out["warnings"][0]["text"].as_str().unwrap();
    assert!(warning.contains("1 commit on no branch"), "{warning}");
    assert!(warning.contains(&backup), "{warning}");
    assert_eq!(repo.rev("HEAD"), first);

    // Undo back to the loose commit; `branch.checkout` from there saves it again.
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("HEAD"), loose);
    let out = run(&r, &mut s, "branch.checkout", json!({ "name": "main" }));
    let second_backup = out["backup"].as_str().unwrap();
    assert_ne!(second_backup, backup);
    assert_eq!(out["warnings"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(detached_backups(&repo).len(), 2);
    assert_eq!(head_branch(&repo).as_deref(), Some("refs/heads/main"));
}

#[test]
fn leaving_a_commit_on_a_branch_tag_or_remote_saves_nothing() {
    let (repo, first, second) = history();
    let remote = Repo::bare();
    repo.add_remote("origin", &remote);
    repo.detach(second);
    let pushed = repo.commit_file("p.txt", "p\n", "pushed");
    repo.push("origin", "HEAD", "topic");

    let (r, mut s) = open(&repo);
    // `pushed` is only on `origin/topic`.
    let out = run(&r, &mut s, "commit.checkout", json!({ "rev": "v1" }));
    assert!(out.get("backup").is_none(), "{out}");
    // `second` is tagged, `first` is on `main`.
    run(
        &r,
        &mut s,
        "commit.checkout",
        json!({ "rev": first.to_string() }),
    );
    let out = run(&r, &mut s, "branch.checkout", json!({ "name": "main" }));
    assert!(out.get("backup").is_none(), "{out}");
    assert_eq!(out["warnings"], json!([]));
    assert!(detached_backups(&repo).is_empty());
    assert_eq!(repo.rev("refs/remotes/origin/topic"), pushed);
}

#[test]
fn a_failed_backup_switches_nothing() {
    let (repo, first, _, loose) = with_loose();
    // `refs/project/backup` is a ref: nothing can be created under it.
    repo.git(&["update-ref", "refs/project/backup", &first.to_string()]);
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "commit.checkout",
            &mut s,
            json!({ "rev": first.to_string() }),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Git { .. } | EngineError::GitWithReason { .. }
        ),
        "{err}"
    );
    assert_eq!(repo.rev("HEAD"), loose);
    assert_eq!(head_branch(&repo), None);
    assert!(!s.journal().unwrap().can_undo());
    let err = r
        .run("branch.checkout", &mut s, json!({ "name": "main" }))
        .unwrap_err();
    assert!(
        !matches!(err, EngineError::CheckoutConflict { .. }),
        "{err}"
    );
    assert_eq!(repo.rev("HEAD"), loose);
}

#[test]
fn a_refused_switch_drops_the_backup_and_a_failing_hook_keeps_it() {
    // Local changes refuse the switch: HEAD is still on the loose commit, backup removed.
    let (repo, first, _, loose) = with_loose();
    repo.write("a.txt", "my change\n");
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "commit.checkout",
            &mut s,
            json!({ "rev": first.to_string() }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::CheckoutConflict { .. }), "{err}");
    assert_eq!(repo.rev("HEAD"), loose);
    assert!(detached_backups(&repo).is_empty());

    // A `post-checkout` hook that fails after the switch: HEAD moved, the backup stays and the
    // error names it.
    let (repo, first, _, loose) = with_loose();
    install_hook(&repo, "post-checkout", "exit 1\n");
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "commit.checkout",
            &mut s,
            json!({ "rev": first.to_string() }),
        )
        .unwrap_err();
    assert_eq!(repo.rev("HEAD"), first);
    let kept = detached_backups(&repo);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].1, loose);
    assert!(err.to_string().contains(&kept[0].0), "{err}");
}
