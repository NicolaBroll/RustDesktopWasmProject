//! Integration tests of the `branch.*` commands (S24) with the `CliBackend` on real repos:
//! creation, checkout (also refused because of local changes, with the files intact), deletion
//! (with confirmation if not merged, backup, undo and redo), rename and undo; names starting
//! with `-` (even a branch called `--force`), additional worktrees.

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

/// All refs with their value and HEAD, to check that nothing changes.
fn refs(repo: &Repo) -> String {
    let head = repo.git_output(&["symbolic-ref", "-q", "HEAD"]).stdout;
    format!(
        "{}\nHEAD -> {}{}",
        repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
        String::from_utf8_lossy(&head).trim(),
        repo.git(&["rev-parse", "-q", "--verify", "HEAD"])
    )
}

/// The value of the ref `name` (full name), `None` if it does not exist.
fn ref_value(repo: &Repo, name: &str) -> Option<Oid> {
    let out = repo.git_output(&["rev-parse", "-q", "--verify", "--end-of-options", name]);
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().parse().unwrap())
}

fn head_branch(repo: &Repo) -> String {
    repo.git(&["symbolic-ref", "HEAD"])
}

/// Index, `git status` and contents of the given files: everything a refused checkout must
/// not touch.
fn snapshot(repo: &Repo, files: &[&str]) -> (String, String, String, Vec<Option<Vec<u8>>>) {
    (
        refs(repo),
        repo.git(&["ls-files", "-s"]),
        repo.git(&["status", "--porcelain=v1", "--ignored", "-uall"]),
        files
            .iter()
            .map(|f| std::fs::read(repo.path().join(f)).ok())
            .collect(),
    )
}

/// `main` with `a.txt`, and `other` (one commit later) that changes `a.txt` and adds `b.txt`;
/// HEAD on `main`.
fn two_branches() -> (Repo, Oid, Oid) {
    let repo = Repo::new();
    let main = repo.commit_file("a.txt", "a\n", "first");
    repo.branch("other");
    repo.checkout("other");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    let other_tip = repo.commit_file("a.txt", "a on other\n", "other");
    repo.checkout("main");
    (repo, main, other_tip)
}

#[test]
fn create_from_head_and_from_a_rev_never_overwrites() {
    let (repo, main, other_tip) = two_branches();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "branch.create", json!({ "name": "new" }));
    assert_eq!(out["branch"], "refs/heads/new");
    assert_eq!(out["target"], main.to_string());
    assert!(
        out["refs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "refs/heads/new")
    );
    assert_eq!(ref_value(&repo, "refs/heads/new"), Some(main));
    assert_eq!(head_branch(&repo), "refs/heads/main");
    // Reflog with our message.
    let log = repo.git(&["reflog", "show", "--format=%gs", "refs/heads/new"]);
    assert!(log.starts_with("project: branch.create new"), "{log}");

    let out = run(
        &r,
        &mut s,
        "branch.create",
        json!({ "name": "from-other", "start": "other" }),
    );
    assert_eq!(out["target"], other_tip.to_string());

    // A symbolic ref to a ref that does not exist: not followed (it would create `refs/heads/target`).
    repo.git(&["symbolic-ref", "refs/heads/dang", "refs/heads/target"]);
    // Existing, contained in another (`other/x`), containing one (`new` -> `new`
    // exists), unknown rev: nothing changes.
    let before = refs(&repo);
    for (params, kind) in [
        (json!({ "name": "other" }), "NotAllowed"),
        (json!({ "name": "other/x" }), "NotAllowed"),
        (json!({ "name": "dang" }), "NotAllowed"),
        (json!({ "name": "x", "start": "nobody" }), "UnknownRev"),
        (json!({ "name": "-x" }), "InvalidParams"),
        (json!({ "name": "--force" }), "InvalidParams"),
        (json!({ "name": "@" }), "InvalidParams"),
        (json!({ "name": "a b" }), "InvalidParams"),
    ] {
        let err = r.run("branch.create", &mut s, params.clone()).unwrap_err();
        let got = match err {
            EngineError::NotAllowed(_) => "NotAllowed",
            EngineError::Git { .. } => "Git",
            EngineError::UnknownRev(_) => "UnknownRev",
            EngineError::InvalidParams { .. } => "InvalidParams",
            ref other => panic!("{params}: {other}"),
        };
        assert_eq!(got, kind, "{params}: {err}");
    }
    assert_eq!(refs(&repo), before);
    assert_eq!(ref_value(&repo, "refs/heads/target"), None);
    assert_eq!(
        repo.git(&["symbolic-ref", "refs/heads/dang"]),
        "refs/heads/target"
    );

    // Undo deletes the branch, redo recreates it.
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/from-other"), None);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/from-other"), Some(other_tip));
}

#[test]
fn checkout_and_undo() {
    let (repo, main, other_tip) = two_branches();
    // A change that does not touch the differing files stays.
    repo.write("c.txt", "untracked\n");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "branch.checkout", json!({ "name": "other" }));
    assert_eq!(out["branch"], "refs/heads/other");
    assert_eq!(out["previous"], json!({ "branch": "refs/heads/main" }));
    assert_eq!(out["changed"], true);
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(out["status"]["head"]["Branch"]["name"], "other");
    assert_eq!(head_branch(&repo), "refs/heads/other");
    assert_eq!(repo.rev("HEAD"), other_tip);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a on other\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("c.txt")).unwrap(),
        "untracked\n"
    );

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["undone"]["command"], "branch.checkout");
    assert_eq!(head_branch(&repo), "refs/heads/main");
    assert_eq!(repo.rev("HEAD"), main);
    assert!(!repo.path().join("b.txt").exists());
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(head_branch(&repo), "refs/heads/other");

    // Undo refused if HEAD was changed from outside.
    repo.checkout("main");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(head_branch(&repo), "refs/heads/main");
}

#[test]
fn checkout_from_detached_head_is_undone_to_the_same_commit() {
    let (repo, main, other_tip) = two_branches();
    repo.detach(main);
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "branch.checkout", json!({ "name": "other" }));
    assert_eq!(out["previous"], json!({ "detached": main.to_string() }));
    assert_eq!(repo.rev("HEAD"), other_tip);
    run(&r, &mut s, "edit.undo", json!({}));
    assert!(
        !repo
            .git_output(&["symbolic-ref", "-q", "HEAD"])
            .status
            .success()
    );
    assert_eq!(repo.rev("HEAD"), main);
}

#[test]
fn checkout_refused_by_local_changes_leaves_everything_intact() {
    let files = ["a.txt", "b.txt", "ign.txt"];

    // Unstaged change, then staged, to a file that differs in the target branch.
    for staged in [false, true] {
        let (repo, _, _) = two_branches();
        repo.write("a.txt", "my change\n");
        if staged {
            repo.stage(&["a.txt"]);
        }
        let before = snapshot(&repo, &files);
        let (r, mut s) = open(&repo);
        let err = r
            .run("branch.checkout", &mut s, json!({ "name": "other" }))
            .unwrap_err();
        let EngineError::CheckoutConflict {
            target,
            files: hit,
            stderr,
        } = &err
        else {
            panic!("{err}");
        };
        assert_eq!(target, "other");
        assert_eq!(hit, &["a.txt"], "{stderr}");
        assert!(err.to_string().contains("Nothing changed"), "{err}");
        assert_eq!(snapshot(&repo, &files), before);
        assert!(!s.journal().unwrap().can_undo());
    }

    // An untracked and an ignored file that the target branch contains.
    let (repo, _, _) = two_branches();
    repo.checkout("other");
    repo.commit_file("ign.txt", "branch version\n", "ign");
    repo.checkout("main");
    std::fs::write(repo.path().join(".git/info/exclude"), "ign.txt\n").unwrap();
    repo.write("ign.txt", "mine, ignored\n");
    repo.write("b.txt", "mine, untracked\n");
    let before = snapshot(&repo, &files);
    let (r, mut s) = open(&repo);
    let err = r
        .run("branch.checkout", &mut s, json!({ "name": "other" }))
        .unwrap_err();
    let EngineError::CheckoutConflict { files: hit, .. } = &err else {
        panic!("{err}");
    };
    assert_eq!(hit, &["b.txt", "ign.txt"]);
    assert_eq!(snapshot(&repo, &files), before);
}

#[test]
fn checkout_refused_during_a_merge_or_with_assume_unchanged_files() {
    let (repo, _, _) = two_branches();
    repo.branch("third");
    repo.commit_file("a.txt", "a on main\n", "main");
    repo.conflict("other");
    let before = snapshot(&repo, &["a.txt"]);
    let (r, mut s) = open(&repo);
    let err = r
        .run("branch.checkout", &mut s, json!({ "name": "third" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(snapshot(&repo, &["a.txt"]), before);

    // `--assume-unchanged`: git would overwrite the change without saying so.
    let (repo, _, _) = two_branches();
    repo.git(&["update-index", "--assume-unchanged", "a.txt"]);
    repo.write("a.txt", "hidden change\n");
    let (r, mut s) = open(&repo);
    let err = r
        .run("branch.checkout", &mut s, json!({ "name": "other" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "hidden change\n"
    );
    assert_eq!(head_branch(&repo), "refs/heads/main");
}

#[test]
fn names_starting_with_a_dash() {
    let (repo, main, other_tip) = two_branches();
    // Branches that git would not let you create, but that can exist.
    repo.git(&["update-ref", "refs/heads/-x", &other_tip.to_string()]);
    repo.git(&["update-ref", "refs/heads/--force", &other_tip.to_string()]);
    repo.git(&["update-ref", "refs/heads/-d", &other_tip.to_string()]);
    let (r, mut s) = open(&repo);

    // A `--force` branch forces nothing: the local change blocks the checkout.
    repo.write("a.txt", "my change\n");
    let err = r
        .run("branch.checkout", &mut s, json!({ "name": "--force" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::CheckoutConflict { .. }), "{err}");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "my change\n"
    );
    assert_eq!(head_branch(&repo), "refs/heads/main");
    repo.git(&["checkout", "--", "a.txt"]);

    run(&r, &mut s, "branch.checkout", json!({ "name": "-x" }));
    assert_eq!(head_branch(&repo), "refs/heads/-x");
    run(&r, &mut s, "branch.checkout", json!({ "name": "main" }));

    // `-d` does not become "delete": only `-d` is deleted.
    let out = run(
        &r,
        &mut s,
        "branch.delete",
        json!({ "name": "-d", "confirm": true }),
    );
    assert_eq!(ref_value(&repo, "refs/heads/-d"), None);
    assert_eq!(ref_value(&repo, "refs/heads/main"), Some(main));
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(ref_value(&repo, backup), Some(other_tip));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/-d"), Some(other_tip));

    // Rename of `--force` to a valid name.
    run(
        &r,
        &mut s,
        "branch.rename",
        json!({ "name": "--force", "new_name": "forced" }),
    );
    assert_eq!(ref_value(&repo, "refs/heads/--force"), None);
    assert_eq!(ref_value(&repo, "refs/heads/forced"), Some(other_tip));

    // A new name with `-` is refused before reaching git.
    let before = refs(&repo);
    for (id, params) in [
        ("branch.create", json!({ "name": "-n" })),
        ("branch.rename", json!({ "name": "-x", "new_name": "-y" })),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{id}: {err}"
        );
    }
    assert_eq!(refs(&repo), before);
}

#[test]
fn delete_unmerged_needs_confirm_and_undo_recreates() {
    let (repo, main, other_tip) = two_branches();
    repo.git(&["config", "branch.other.description", "to keep?"]);
    let (r, mut s) = open(&repo);
    let before = refs(&repo);
    let err = r
        .run("branch.delete", &mut s, json!({ "name": "other" }))
        .unwrap_err();
    let EngineError::ConfirmRequired {
        command, reason, ..
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(*command, "branch.delete");
    assert!(reason.contains("1 commit"), "{reason}");
    assert_eq!(refs(&repo), before);
    assert!(!s.journal().unwrap().can_undo());

    let out = run(
        &r,
        &mut s,
        "branch.delete",
        json!({ "name": "other", "confirm": true }),
    );
    assert_eq!(out["target"], other_tip.to_string());
    assert_eq!(out["unmerged"], 1);
    assert_eq!(out["warnings"], json!([]));
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(
        backup.starts_with("refs/project/backup/delete-"),
        "{backup}"
    );
    assert_eq!(ref_value(&repo, &backup), Some(other_tip));
    assert_eq!(ref_value(&repo, "refs/heads/other"), None);
    // Like `git branch -d`: the branch configuration is removed.
    assert!(
        !repo
            .git_output(&["config", "--get", "branch.other.description"])
            .status
            .success()
    );
    // The backup survives gc.
    repo.git(&["gc", "--quiet", "--prune=now"]);
    assert_eq!(
        repo.git(&["cat-file", "-t", &other_tip.to_string()]),
        "commit"
    );

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["undone"]["command"], "branch.delete");
    assert_eq!(out["backups"], json!([]));
    assert_eq!(ref_value(&repo, "refs/heads/other"), Some(other_tip));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/other"), None);

    // Recreated from outside on another commit: the undo does not overwrite it.
    repo.git(&["branch", "other", &main.to_string()]);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(ref_value(&repo, "refs/heads/other"), Some(main));
}

#[test]
fn delete_merged_current_and_open_in_another_worktree() {
    let (repo, main, _) = two_branches();
    repo.branch("merged");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "branch.delete", json!({ "name": "merged" }));
    assert_eq!(out["unmerged"], 0);
    assert_eq!(ref_value(&repo, "refs/heads/merged"), None);
    assert_eq!(
        ref_value(&repo, out["backup"].as_str().unwrap()),
        Some(main)
    );

    let before = refs(&repo);
    let err = r
        .run(
            "branch.delete",
            &mut s,
            json!({ "name": "main", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let err = r
        .run(
            "branch.delete",
            &mut s,
            json!({ "name": "nobody", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(refs(&repo), before);

    // `other` open in another worktree: refused, nothing changes (not even a backup).
    let wt = tempfile::tempdir().unwrap();
    let wt_path = wt.path().join("wt");
    repo.git(&[
        "worktree",
        "add",
        "--quiet",
        wt_path.to_str().unwrap(),
        "other",
    ]);
    let before_wt = refs(&repo);
    let err = r
        .run(
            "branch.delete",
            &mut s,
            json!({ "name": "other", "confirm": true }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("worktree")),
        "{err}"
    );
    assert_eq!(refs(&repo), before_wt);

    // Not even the redo of a deletion removes a branch open elsewhere.
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/merged"), Some(main));
    let wt2 = wt.path().join("wt2");
    repo.git(&[
        "worktree",
        "add",
        "--quiet",
        wt2.to_str().unwrap(),
        "merged",
    ]);
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("worktree")),
        "{err}"
    );
    assert_eq!(ref_value(&repo, "refs/heads/merged"), Some(main));
}

#[test]
fn delete_merged_into_its_upstream_needs_no_confirm() {
    let (repo, _, other_tip) = two_branches();
    let remote = Repo::bare();
    repo.add_remote("origin", &remote);
    repo.push("origin", "other", "other");
    repo.set_upstream("other", "origin");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "branch.delete", json!({ "name": "other" }));
    assert_eq!(out["unmerged"], 0);
    assert_eq!(
        ref_value(&repo, "refs/remotes/origin/other"),
        Some(other_tip)
    );
}

#[test]
fn rename_and_undo() {
    let (repo, main, _) = two_branches();
    let remote = Repo::bare();
    repo.add_remote("origin", &remote);
    repo.push("origin", "main", "main");
    repo.set_upstream("main", "origin");
    let reflog_before = repo.git(&["reflog", "show", "--format=%H", "refs/heads/main"]);
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.rename",
        json!({ "name": "main", "new_name": "primary" }),
    );
    assert_eq!(out["branch"], "refs/heads/primary");
    assert_eq!(out["previous"], "refs/heads/main");
    assert_eq!(out["target"], main.to_string());
    assert_eq!(out["head"], true);
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(head_branch(&repo), "refs/heads/primary");
    assert_eq!(ref_value(&repo, "refs/heads/main"), None);
    assert_eq!(ref_value(&repo, "refs/heads/primary"), Some(main));
    // Upstream and reflog follow the branch.
    assert_eq!(
        repo.git(&["config", "branch.primary.merge"]),
        "refs/heads/main"
    );
    let reflog = repo.git(&["reflog", "show", "--format=%H", "refs/heads/primary"]);
    assert!(reflog.ends_with(&reflog_before), "{reflog}");

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["undone"]["command"], "branch.rename");
    assert_eq!(head_branch(&repo), "refs/heads/main");
    assert_eq!(ref_value(&repo, "refs/heads/main"), Some(main));
    assert_eq!(ref_value(&repo, "refs/heads/primary"), None);
    assert_eq!(repo.git(&["config", "branch.main.remote"]), "origin");
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(head_branch(&repo), "refs/heads/primary");

    // Refusals without changing anything: name already taken, missing branch.
    let before = refs(&repo);
    for params in [
        json!({ "name": "primary", "new_name": "other" }),
        json!({ "name": "nobody", "new_name": "x" }),
    ] {
        let err = r.run("branch.rename", &mut s, params.clone()).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{params}: {err}");
    }
    assert_eq!(refs(&repo), before);

    // The old name taken again from outside: the undo is refused.
    repo.git(&["branch", "main", &main.to_string()]);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(head_branch(&repo), "refs/heads/primary");
}

#[test]
fn rename_of_another_branch_keeps_head() {
    let (repo, _, other_tip) = two_branches();
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.rename",
        json!({ "name": "other", "new_name": "feature/other" }),
    );
    assert_eq!(out["head"], false);
    assert_eq!(head_branch(&repo), "refs/heads/main");
    assert_eq!(
        ref_value(&repo, "refs/heads/feature/other"),
        Some(other_tip)
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/other"), Some(other_tip));
    assert_eq!(ref_value(&repo, "refs/heads/feature/other"), None);
}

#[test]
fn checkout_refused_when_a_folder_with_untracked_files_would_become_a_file() {
    let (repo, _, _) = two_branches();
    repo.checkout("other");
    repo.commit_file("d", "file d in the branch\n", "d");
    repo.checkout("main");
    repo.write("d/x", "mine, untracked\n");
    let before = snapshot(&repo, &["d/x"]);
    let (r, mut s) = open(&repo);
    let err = r
        .run("branch.checkout", &mut s, json!({ "name": "other" }))
        .unwrap_err();
    let EngineError::CheckoutConflict { files, stderr, .. } = &err else {
        panic!("{err}");
    };
    assert!(stderr.contains("would lose untracked files"), "{stderr}");
    assert!(files.iter().any(|f| f.starts_with('d')), "{files:?}");
    assert_eq!(snapshot(&repo, &["d/x"]), before);
}

#[test]
fn delete_saves_the_branch_config_and_undo_rewrites_it() {
    let (repo, _, other_tip) = two_branches();
    let remote = Repo::bare();
    repo.add_remote("origin", &remote);
    repo.push("origin", "main", "other-remote");
    repo.git(&["config", "branch.other.remote", "origin"]);
    repo.git(&["config", "branch.other.merge", "refs/heads/other-remote"]);
    repo.git(&["config", "branch.other.description", "-starts with a dash"]);
    let config = |repo: &Repo| {
        let out = repo.git_output(&["config", "--local", "--get-regexp", "^branch\\.other\\."]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let saved = config(&repo);
    assert_eq!(saved.lines().count(), 3, "{saved}");
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.delete",
        json!({ "name": "other", "confirm": true }),
    );
    assert_eq!(
        out["removed_config"],
        json!([
            { "key": "branch.other.remote", "value": "origin" },
            { "key": "branch.other.merge", "value": "refs/heads/other-remote" },
            { "key": "branch.other.description", "value": "-starts with a dash" }
        ])
    );
    assert_eq!(config(&repo), "");
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"][0]["branch_config"], out["removed_config"]);

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(ref_value(&repo, "refs/heads/other"), Some(other_tip));
    assert_eq!(config(&repo), saved);
    // The upstream works again.
    let refs = run(&r, &mut s, "repo.refs", json!({}));
    let branch = refs
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "refs/heads/other")
        .unwrap();
    assert_eq!(
        branch["upstream"]["name"],
        "refs/remotes/origin/other-remote"
    );

    let out = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(config(&repo), "");

    // A new configuration that appeared in the meantime is not overwritten: warning.
    repo.git(&["config", "branch.other.description", "new"]);
    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/other"), Some(other_tip));
    assert_eq!(out["warnings"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(
        repo.git(&["config", "--get-all", "branch.other.description"]),
        "new"
    );
    // And the redo does not remove it.
    let out = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/other"), None);
    assert_eq!(out["warnings"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(repo.git(&["config", "branch.other.description"]), "new");
}

#[test]
fn delete_with_expected_refuses_a_branch_that_moved() {
    let (repo, main, other_tip) = two_branches();
    let (r, mut s) = open(&repo);
    // Seen on `other`, then moved from outside.
    repo.git(&["branch", "-f", "other", &main.to_string()]);
    let before = refs(&repo);
    let err = r
        .run(
            "branch.delete",
            &mut s,
            json!({ "name": "other", "confirm": true, "expected": other_tip.to_string() }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains(&main.to_string())),
        "{err}"
    );
    assert_eq!(refs(&repo), before);
    assert!(!s.journal().unwrap().can_undo());
    let err = r
        .run(
            "branch.delete",
            &mut s,
            json!({ "name": "other", "expected": &main.to_string()[..7] }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");

    let out = run(
        &r,
        &mut s,
        "branch.delete",
        json!({ "name": "other", "expected": main.to_string() }),
    );
    assert_eq!(out["target"], main.to_string());
    assert_eq!(ref_value(&repo, "refs/heads/other"), None);
}

#[test]
fn create_does_not_follow_a_dangling_symref() {
    let (repo, _, _) = two_branches();
    repo.git(&["symbolic-ref", "refs/heads/dang", "refs/heads/target"]);
    let (r, mut s) = open(&repo);
    let before = refs(&repo);
    let err = r
        .run("branch.create", &mut s, json!({ "name": "dang" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(refs(&repo), before);
    assert_eq!(ref_value(&repo, "refs/heads/target"), None);
    assert_eq!(
        repo.git(&["symbolic-ref", "refs/heads/dang"]),
        "refs/heads/target"
    );
    assert!(!s.journal().unwrap().can_undo());
}
