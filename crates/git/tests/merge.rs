//! Integration tests of `merge.start`, `merge.continue`, `merge.abort` and `repo.operation`
//! (S29) with the `CliBackend`: fast-forward, merge commit, conflict that stays in progress (the
//! S07 scenario, `repo_with_merge_conflict`, and one started here), continue with conflicts
//! rejected, resolution and continue, abort with and without confirmation and its backup, local
//! changes, untracked or ignored files in place of incoming ones, detached HEAD, undo and
//! redo, names that start with `-`, rebase and cherry-pick in progress seen by `repo.operation`.

mod common;

use common::{Repo, repo_with_divergent_branches, repo_with_merge_conflict};
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

fn read(repo: &Repo, file: &str) -> Option<String> {
    std::fs::read_to_string(repo.path().join(file)).ok()
}

fn backups(repo: &Repo, kind: &str) -> Vec<String> {
    let out = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        &format!("refs/project/backup/{kind}-*"),
    ]);
    out.lines().map(str::to_owned).collect()
}

fn clean(repo: &Repo) -> bool {
    repo.git(&["status", "--porcelain"]).is_empty()
}

fn merging(repo: &Repo) -> bool {
    repo.path().join(".git/MERGE_HEAD").exists()
}

fn parents(repo: &Repo, rev: &str) -> Vec<Oid> {
    repo.git(&["rev-list", "--parents", "-n", "1", rev])
        .split_whitespace()
        .skip(1)
        .map(|h| h.parse().unwrap())
        .collect()
}

/// `main` on `c1` (`a.txt`), `ahead` one commit later (`b.txt`): a fast-forward.
fn ahead_branch() -> (Repo, Oid, Oid) {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "one\n", "first");
    repo.branch("ahead");
    repo.checkout("ahead");
    let c2 = repo.commit_file("b.txt", "b\n", "second");
    repo.checkout("main");
    (repo, c1, c2)
}

#[test]
fn fast_forward_undo_and_redo() {
    let (repo, c1, c2) = ahead_branch();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "ahead" }));
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(out["before"], c1.to_string());
    assert_eq!(out["after"], c2.to_string());
    assert_eq!(out["in_progress"], false);
    assert_eq!(repo.rev("main"), c2);
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("b\n"));
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(repo.rev(backup), c1);
    // The reflog says who moved the branch.
    let reflog = repo.git(&["reflog", "-1", "--format=%gs", "main"]);
    assert!(reflog.starts_with("project: merge.start"), "{reflog}");

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c1);
    assert!(read(&repo, "b.txt").is_none());
    assert!(clean(&repo));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), c2);
    assert!(clean(&repo));

    // Already contained: nothing to do, no backup nor journal.
    let before = backups(&repo, "merge").len();
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "ahead" }));
    assert_eq!(out["strategy"], "up_to_date");
    assert_eq!(backups(&repo, "merge").len(), before);
}

#[test]
fn merge_commit_with_default_and_custom_message() {
    let (repo, c1, c2) = ahead_branch();
    // `main` moves ahead on its own: diverged, without conflicts.
    let m1 = repo.commit_file("c.txt", "c\n", "on main");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "ahead" }));
    assert_eq!(out["strategy"], "merge");
    let merge: Oid = out["after"].as_str().unwrap().parse().unwrap();
    assert_eq!(parents(&repo, "HEAD"), [m1, c2]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B", "HEAD"]),
        "Merge branch 'ahead'"
    );
    assert!(clean(&repo));
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("b\n"));
    assert_eq!(repo.rev(out["backup"].as_str().unwrap()), m1);

    // Undo: the branch goes back with index and worktree; the merge commit stays in a
    // backup.
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), m1);
    assert!(read(&repo, "b.txt").is_none());
    assert!(clean(&repo));
    let kept = undo["backups"][0].as_str().unwrap();
    assert_eq!(repo.rev(kept), merge);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), merge);
    run(&r, &mut s, "edit.undo", json!({}));

    // `no_ff` and a chosen message, on a branch that is not `main`.
    repo.branch("dev");
    repo.checkout("dev");
    repo.git(&["reset", "--quiet", "--hard", &c1.to_string()]);
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "ahead", "no_ff": true, "message": "Merge\n\nwith body" }),
    );
    assert_eq!(out["strategy"], "merge");
    assert_eq!(parents(&repo, "dev"), [c1, c2]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B", "dev"]),
        "Merge\n\nwith body"
    );
    run(&r, &mut s, "edit.undo", json!({}));
    // The default message with ` into <branch>`.
    run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "ahead", "no_ff": true }),
    );
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s", "dev"]),
        "Merge branch 'ahead' into dev"
    );
}

#[test]
fn ff_only_refuses_divergent_branches() {
    let (repo, theirs) = repo_with_divergent_branches();
    let head = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "merge.start",
            &mut s,
            json!({ "rev": "other", "ff_only": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(repo.rev("HEAD"), head);
    assert!(backups(&repo, "merge").is_empty());
    assert_ne!(theirs, head);
}

#[test]
fn conflict_stays_in_progress_then_continue_and_undo() {
    let (repo, theirs) = repo_with_divergent_branches();
    let ours = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "other" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["after"], ours.to_string());
    assert_eq!(out["can_undo"], false);
    assert_eq!(out["status"]["operation"], "Merge");
    assert!(
        out["message"]
            .as_str()
            .unwrap()
            .contains("Merge conflict in a.txt")
    );
    assert!(merging(&repo));
    assert!(read(&repo, "a.txt").unwrap().contains("<<<<<<<"));
    // The backup stays: the merge is in progress.
    assert_eq!(repo.rev(out["backup"].as_str().unwrap()), ours);

    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Merge");
    assert_eq!(op["conflicts"], json!(["a.txt"]));
    assert_eq!(op["incoming"][0]["id"], theirs.to_string());
    assert_eq!(op["incoming"][0]["label"], "other");
    assert_eq!(op["incoming"][0]["summary"], "a.txt on other");
    assert_eq!(op["message"], "Merge branch 'other'");
    assert_eq!(op["ready"], false);
    assert_eq!(op["continue"], "merge.continue");
    assert_eq!(op["abort"], "merge.abort");

    // With the merge in progress: no undo, no other merge, no continue with conflicts.
    for (id, params) in [
        ("edit.undo", json!({})),
        ("merge.start", json!({ "rev": "other" })),
        ("merge.continue", json!({})),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
    }
    let err = r.run("merge.continue", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    assert!(merging(&repo));

    repo.write("a.txt", "resolved\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    assert_eq!(run(&r, &mut s, "repo.operation", json!({}))["ready"], true);
    let out = run(&r, &mut s, "merge.continue", json!({}));
    // The `MERGE_MSG` message without the `# Conflicts:` block.
    assert_eq!(out["message"], "Merge branch 'other'");
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B", "HEAD"]),
        "Merge branch 'other'"
    );
    assert_eq!(parents(&repo, "HEAD"), [ours, theirs]);
    assert!(!merging(&repo));
    assert!(clean(&repo));
    assert_eq!(out["status"]["operation"], Value::Null);
    let merge = repo.rev("HEAD");

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), ours);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("ours\n"));
    assert!(clean(&repo));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), merge);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("resolved\n"));
}

#[test]
fn scenario_of_s07_from_the_terminal() {
    // The conflicted merge started from a terminal (S07): status, banner, continue with a
    // chosen message.
    let repo = repo_with_merge_conflict();
    let ours = repo.rev("HEAD");
    let theirs = repo.rev("other");
    let (r, mut s) = open(&repo);
    assert_eq!(
        run(&r, &mut s, "repo.status", json!({}))["operation"],
        "Merge"
    );
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["incoming"][0]["id"], theirs.to_string());
    assert_eq!(op["incoming"][0]["refs"], json!(["refs/heads/other"]));
    assert_eq!(op["conflicts"], json!(["a.txt"]));

    repo.write("a.txt", "both\n");
    repo.stage(&["a.txt"]);
    let out = run(
        &r,
        &mut s,
        "merge.continue",
        json!({ "message": "Merge other by hand" }),
    );
    assert_eq!(out["commit"]["summary"], "Merge other by hand");
    assert_eq!(parents(&repo, "HEAD"), [ours, theirs]);
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Merge of other")
    );
}

#[test]
fn abort_needs_confirm_and_keeps_a_backup() {
    let repo = repo_with_merge_conflict();
    let head = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    // A half-done resolution and a new staged file: they would be lost.
    repo.write("a.txt", "half-done resolution\n");
    repo.write("new.txt", "new\n");
    repo.stage(&["new.txt"]);
    let err = r.run("merge.abort", &mut s, json!({})).unwrap_err();
    match &err {
        EngineError::ConfirmRequired { reason, .. } => {
            assert!(reason.contains("`a.txt`"), "{reason}");
            assert!(reason.contains("`new.txt`"), "{reason}");
        }
        other => panic!("{other}"),
    }
    assert!(merging(&repo));
    assert!(backups(&repo, "merge-abort").is_empty());

    let out = run(&r, &mut s, "merge.abort", json!({ "confirm": true }));
    assert!(!merging(&repo));
    assert!(clean(&repo));
    assert_eq!(repo.rev("HEAD"), head);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("ours\n"));
    assert_eq!(out["status"]["operation"], Value::Null);
    assert_eq!(out["can_undo"], false);
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(backups(&repo, "merge-abort"), [backup]);
    // The backup is like a stash: worktree (parents HEAD and the index), with the files as they were.
    assert_eq!(
        repo.git(&["show", &format!("{backup}:a.txt")]),
        "half-done resolution"
    );
    assert_eq!(repo.git(&["show", &format!("{backup}:new.txt")]), "new");
    let p = parents(&repo, backup);
    assert_eq!(p[0], head);
    assert_eq!(repo.git(&["show", &format!("{}:new.txt", p[1])]), "new");
    // In the saved index the conflicted file has our version.
    assert_eq!(repo.git(&["show", &format!("{}:a.txt", p[1])]), "ours");
    // `git stash apply` puts it back.
    repo.git(&["stash", "apply", backup]);
    assert_eq!(
        read(&repo, "a.txt").as_deref(),
        Some("half-done resolution\n")
    );
    assert_eq!(read(&repo, "new.txt").as_deref(), Some("new\n"));

    // Without a merge in progress.
    let err = r
        .run("merge.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(
        err.to_string().contains("a merge is not in progress"),
        "{err}"
    );
}

#[test]
fn abort_with_staged_and_unstaged_changes_is_refused_before_writing() {
    let repo = repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    // A staged file with other unstaged changes: `git merge --abort` would refuse, the
    // backend checks it first (like the `MemoryBackend`) and writes nothing.
    repo.write("new.txt", "staged\n");
    repo.stage(&["new.txt"]);
    repo.write("new.txt", "after\n");
    let err = r
        .run("merge.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`new.txt`"), "{err}");
    assert!(merging(&repo));
    assert_eq!(read(&repo, "new.txt").as_deref(), Some("after\n"));
    assert!(backups(&repo, "merge-abort").is_empty());
}

/// `git merge --abort` that fails halfway (a read-only directory): the backup stays and has
/// the resolution that git may have already deleted.
#[cfg(unix)]
#[test]
fn abort_failing_halfway_keeps_the_backup() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.commit_file("a/x.txt", "base\n", "base a");
    repo.commit_file("z/a.txt", "base\n", "base z");
    repo.branch("other");
    repo.checkout("other");
    repo.commit_file("a/x.txt", "theirs\n", "a theirs");
    repo.commit_file("z/a.txt", "theirs\n", "z theirs");
    repo.checkout("main");
    repo.commit_file("a/x.txt", "ours\n", "a ours");
    repo.commit_file("z/a.txt", "ours\n", "z ours");
    repo.conflict("other");
    repo.write("a/x.txt", "resolved but not staged\n");
    let z = repo.path().join("z");
    std::fs::set_permissions(&z, std::fs::Permissions::from_mode(0o555)).unwrap();
    // As root permissions do not block writing: the halfway failure cannot be reproduced.
    if std::fs::write(z.join("probe"), "").is_ok() {
        std::fs::set_permissions(&z, std::fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("skipped: the read-only directory is writable (root?)");
        return;
    }
    let (r, mut s) = open(&repo);
    let result = r.run("merge.abort", &mut s, json!({ "confirm": true }));
    std::fs::set_permissions(&z, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = result.unwrap_err();
    let kept = backups(&repo, "merge-abort");
    assert_eq!(kept.len(), 1, "{err}");
    assert!(err.to_string().contains(&kept[0]), "{err}");
    assert!(!err.to_string().contains("nothing changed"), "{err}");
    assert_eq!(
        repo.git(&["show", &format!("{}:a/x.txt", kept[0])]),
        "resolved but not staged"
    );
}

/// A conflict "deleted by us" on an ignored file: the backup saves it anyway (`add -f`)
/// and the abort succeeds.
#[test]
fn abort_with_a_conflict_on_an_ignored_path() {
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignore the logs");
    repo.write("gen.log", "base\n");
    repo.git(&["add", "-f", "gen.log"]);
    repo.commit("gen.log tracked");
    repo.branch("feat");
    repo.checkout("feat");
    repo.commit_file("gen.log", "feat\n", "change the log");
    repo.checkout("main");
    repo.git(&["rm", "--quiet", "gen.log"]);
    repo.commit("remove the log");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "feat" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["gen.log"]));
    let out = run(&r, &mut s, "merge.abort", json!({ "confirm": true }));
    assert!(!merging(&repo));
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(repo.git(&["show", &format!("{backup}:gen.log")]), "feat");
}

/// `branch.<b>.mergeOptions=--squash` does not turn the fast-forward into a squash.
#[test]
fn fast_forward_ignores_merge_options_squash() {
    let (repo, _, c2) = ahead_branch();
    repo.git(&["config", "branch.main.mergeOptions", "--squash"]);
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "ahead" }));
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(repo.rev("main"), c2);
    assert!(clean(&repo));
    assert_eq!(out["can_undo"], true);
}

/// A `post-merge` hook that moves the branch: the merge is done, but it does not enter the journal.
#[cfg(unix)]
#[test]
fn branch_moved_during_the_merge_is_not_journaled() {
    use std::os::unix::fs::PermissionsExt;
    let (repo, _, c2) = ahead_branch();
    let hook = repo.path().join(".git/hooks/post-merge");
    std::fs::write(
        &hook,
        "#!/bin/sh\ngit commit --quiet --allow-empty -m other\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "ahead" }));
    assert_ne!(repo.rev("main"), c2);
    assert_eq!(parents(&repo, "main"), [c2]);
    assert_eq!(out["can_undo"], false);
    assert!(
        out["warnings"].as_array().unwrap().iter().any(|w| w["text"]
            .as_str()
            .unwrap()
            .contains("does not go in the journal")),
        "{out}"
    );
}

/// Undo and redo of a discard are refused with a merge in progress.
#[test]
fn undo_of_a_discard_during_a_merge_is_refused() {
    let (repo, _) = repo_with_divergent_branches();
    repo.commit_file("c.txt", "c\n", "c on main");
    repo.write("c.txt", "modified\n");
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "worktree.discard",
        json!({ "paths": ["c.txt"], "confirm": true }),
    );
    repo.conflict("other");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("a merge"), "{err}");
    assert_eq!(read(&repo, "c.txt").as_deref(), Some("c\n"));
}

#[test]
fn local_changes_and_files_in_the_way() {
    let (repo, _, _) = ahead_branch();
    repo.commit_file("c.txt", "c\n", "on main");
    let head = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    // A change to a tracked file: no merge commit (by choice), nothing written.
    repo.write("a.txt", "modified\n");
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "ahead" }))
        .unwrap_err();
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    assert!(err.to_string().contains("autostash"), "{err}");
    assert_eq!(repo.rev("HEAD"), head);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("modified\n"));
    assert!(backups(&repo, "merge").is_empty());
    repo.git(&["checkout", "--", "a.txt"]);

    // An ignored file in place of one the merge brings: refused, nothing written.
    repo.write(".git/info/exclude", "b.txt\n");
    repo.write("b.txt", "mine, ignored\n");
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "ahead" }))
        .unwrap_err();
    assert!(err.to_string().contains("`b.txt`"), "{err}");
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("mine, ignored\n"));
    assert_eq!(repo.rev("HEAD"), head);
    assert!(backups(&repo, "merge").is_empty());
}

#[test]
fn fast_forward_with_local_changes() {
    let (repo, c1, c2) = ahead_branch();
    let (r, mut s) = open(&repo);
    // A change that the fast-forward does not touch stays.
    repo.write("a.txt", "modified\n");
    run(&r, &mut s, "merge.start", json!({ "rev": "ahead" }));
    assert_eq!(repo.rev("main"), c2);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("modified\n"));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c1);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("modified\n"));

    // An untracked file in place of an incoming one: git refuses, the backup is removed.
    repo.write("b.txt", "mine\n");
    let before = backups(&repo, "merge").len();
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "ahead" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`b.txt`"), "{err}");
    assert_eq!(repo.rev("main"), c1);
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("mine\n"));
    assert_eq!(backups(&repo, "merge").len(), before);
}

#[test]
fn detached_head_unknown_rev_and_dash_names() {
    let (repo, c1, c2) = ahead_branch();
    let (r, mut s) = open(&repo);
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "nobody" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    // A branch that starts with `-` does not become an option.
    repo.git(&["update-ref", "refs/heads/-x", &c2.to_string()]);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "-x" }));
    assert_eq!(out["source"]["name"], "-x");
    assert_eq!(repo.rev("main"), c2);
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c1);
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "-x", "no_ff": true }),
    );
    assert_eq!(out["strategy"], "merge");
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Merge branch '-x'");

    repo.detach(c1);
    let (r, mut s) = open(&repo);
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "ahead" }))
        .unwrap_err();
    assert!(err.to_string().contains("HEAD is detached"), "{err}");
    assert_eq!(repo.rev("HEAD"), c1);
}

#[test]
fn operation_of_a_rebase_and_a_cherry_pick() {
    let (repo, theirs) = repo_with_divergent_branches();
    let ours = repo.rev("main");
    repo.git_fails(&["rebase", "other"]);
    let (r, mut s) = open(&repo);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Rebase");
    assert_eq!(op["onto"], theirs.to_string());
    assert_eq!(op["rebase_branch"], "refs/heads/main");
    assert_eq!(op["conflicts"], json!(["a.txt"]));
    // A rebase started from the terminal is offered to `rebase.continue`/`rebase.abort` (S32).
    assert_eq!(op["continue"], "rebase.continue");
    assert_eq!(op["abort"], "rebase.abort");
    let err = r
        .run("merge.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(err.to_string().contains("a rebase"), "{err}");
    repo.git(&["rebase", "--abort"]);

    repo.git_fails(&["cherry-pick", "other"]);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "CherryPick");
    assert_eq!(op["incoming"][0]["id"], theirs.to_string());
    assert_eq!(op["incoming"][0]["label"], "other");
    assert_eq!(repo.rev("main"), ours);

    repo.git(&["cherry-pick", "--abort"]);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], Value::Null);
    assert_eq!(op["incoming"], json!([]));
    assert_eq!(op["message"], Value::Null);
    assert_eq!(op["ready"], false);
}
