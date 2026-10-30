//! Integration tests of `edit.undo`, `edit.redo` and `edit.history` (operation journal,
//! §7) with the `CliBackend` on real repos: undo and redo of a commit, of the first
//! commit, of an `amend` and with detached HEAD; refusal if the refs changed from outside; backup
//! of undone commits that survives `git gc`; hooks that reject the transaction.

mod common;

use common::{Repo, repo_detached_head, repo_with_divergent_branches};
use project_engine::{EngineError, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

/// Registry and session with `repo` open, like the CLI.
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

/// All the refs with their value (`for-each-ref`) and HEAD, to check that nothing changes.
fn refs(repo: &Repo) -> String {
    let head = repo.git_output(&["symbolic-ref", "-q", "HEAD"]).stdout;
    format!(
        "{}\nHEAD -> {}{}",
        repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
        String::from_utf8_lossy(&head).trim(),
        repo.git(&["rev-parse", "-q", "--verify", "HEAD"])
    )
}

/// Index (`ls-files -s`) and content of the given worktree files.
fn index_and_files(repo: &Repo, files: &[&str]) -> (String, Vec<Option<String>>) {
    let contents = files
        .iter()
        .map(|f| std::fs::read_to_string(repo.path().join(f)).ok())
        .collect();
    (repo.git(&["ls-files", "-s"]), contents)
}

/// `git status --porcelain` (XY codes and paths).
fn porcelain(repo: &Repo) -> String {
    repo.git(&["status", "--porcelain"])
}

/// A repo with one commit (`a.txt`), a new `b.txt` staged and `a.txt` modified unstaged.
fn repo_ready_to_commit() -> Repo {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "first");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    repo.write("a.txt", "a modified\n");
    repo
}

const FILES: &[&str] = &["a.txt", "b.txt"];

#[test]
fn undo_and_redo_of_a_commit() {
    let repo = repo_ready_to_commit();
    let before = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.create", json!({ "message": "Add b" }));
    let commit = repo.rev("HEAD");
    assert_eq!(out["commit"]["id"], commit.to_string());
    let after_commit = index_and_files(&repo, FILES);

    let out = run(&r, &mut s, "edit.undo", json!({}));
    // The branch goes back to the previous commit; HEAD stays on the branch.
    assert_eq!(repo.rev("refs/heads/main"), before);
    assert_eq!(repo.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    // Index and worktree do not change: the commit's changes go back to staged.
    assert_eq!(index_and_files(&repo, FILES), after_commit);
    assert_eq!(porcelain(&repo), " M a.txt\nA  b.txt");
    assert_eq!(
        out["status"]["head"]["Branch"]["target"],
        before.to_string()
    );
    let undone = &out["undone"];
    assert_eq!(undone["command"], "commit.create");
    assert_eq!(
        undone["description"]["text"],
        format!("Commit {} “Add b”", &commit.to_string()[..7])
    );
    assert_eq!(
        undone["refs"],
        json!([{ "name": "refs/heads/main", "before": before.to_string(), "after": commit.to_string() }])
    );
    assert_eq!(out["can_undo"], false);
    assert_eq!(out["can_redo"], true);
    // The undone commit is in a backup ref.
    let backups = out["backups"].as_array().unwrap();
    assert_eq!(backups.len(), 1);
    let backup = backups[0].as_str().unwrap();
    assert!(backup.starts_with("refs/project/backup/undo-"), "{backup}");
    assert_eq!(repo.rev(backup), commit);
    // Reflog of the branch and of HEAD.
    let message = format!("project: undo Commit {} “Add b”", &commit.to_string()[..7]);
    assert_eq!(
        repo.git(&["log", "-g", "-1", "--format=%gs", "main"]),
        message
    );
    assert_eq!(
        repo.git(&["log", "-g", "-1", "--format=%gs", "HEAD"]),
        message
    );

    let out = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), commit);
    assert_eq!(index_and_files(&repo, FILES), after_commit);
    assert_eq!(porcelain(&repo), " M a.txt");
    assert_eq!(out["redone"]["id"], undone["id"]);
    assert_eq!(
        out["backups"],
        json!([]),
        "the previous commit is the parent"
    );
    assert_eq!(out["redone"]["undo_backups"], json!([backup]));
    assert_eq!(out["can_undo"], true);
    assert_eq!(out["can_redo"], false);
    assert!(
        repo.git(&["log", "-g", "-1", "--format=%gs", "main"])
            .starts_with("project: redo Commit ")
    );
}

#[test]
fn undo_refused_if_the_branch_moved_from_outside() {
    // A `git commit` from a terminal after ours.
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "Add b" }));
    repo.commit_file("c.txt", "c\n", "from terminal");
    let before = refs(&repo);
    let index = index_and_files(&repo, FILES);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    let EngineError::UndoConflict {
        command,
        refs: changed,
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(*command, "edit.undo");
    assert!(changed[0].contains("refs/heads/main"), "{changed:?}");
    assert_eq!(refs(&repo), before, "no ref changed");
    assert_eq!(index_and_files(&repo, FILES), index);
    assert!(
        !repo
            .git(&["for-each-ref", "refs/project/"])
            .contains("undo-")
    );
    // The entry stays in the journal.
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["can_undo"], true);

    // A `git reset` from a terminal.
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "Add b" }));
    repo.git(&["reset", "--quiet", "--soft", "HEAD~1"]);
    let before = refs(&repo);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(refs(&repo), before);

    // HEAD switched to another branch.
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "Add b" }));
    repo.git(&["switch", "--quiet", "-c", "other"]);
    let before = refs(&repo);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("HEAD"), "{err}");
    assert_eq!(refs(&repo), before);
}

#[test]
fn redo_refused_if_the_branch_moved_after_undo() {
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "Add b" }));
    run(&r, &mut s, "edit.undo", json!({}));
    // From a terminal: another commit with what is staged.
    repo.commit("from terminal");
    let before = refs(&repo);
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::UndoConflict {
                command: "edit.redo",
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(refs(&repo), before);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["can_redo"], true);
}

#[test]
fn undo_of_the_first_commit_leaves_an_unborn_branch() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.stage(&["a.txt"]);
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.create", json!({ "message": "first" }));
    assert_eq!(out["previous"], Value::Null);
    let first = repo.rev("HEAD");
    let index = index_and_files(&repo, &["a.txt"]);

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert!(
        !repo
            .git_output(&["rev-parse", "-q", "--verify", "refs/heads/main"])
            .status
            .success(),
        "the branch is deleted"
    );
    assert_eq!(repo.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(index_and_files(&repo, &["a.txt"]), index);
    assert_eq!(porcelain(&repo), "A  a.txt");
    assert_eq!(out["status"]["head"]["Branch"]["target"], Value::Null);
    assert_eq!(out["warnings"], json!([]));
    let backup = out["backups"][0].as_str().unwrap();
    assert_eq!(repo.rev(backup), first);
    // `repo.log` on a branch without commits is empty.
    let log = run(&r, &mut s, "repo.log", json!({}));
    assert_eq!(log["commits"], json!([]));

    let out = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(out["warnings"], json!([]), "no other process: no warning");
    assert_eq!(repo.rev("refs/heads/main"), first);
    assert_eq!(porcelain(&repo), "");
}

#[test]
fn undo_of_an_amend_returns_to_the_rewritten_commit() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "first");
    let old = repo.commit_file("b.txt", "b\n", "second");
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "second, with c", "amend": true }),
    );
    let amended = repo.rev("HEAD");
    assert_eq!(out["previous"], old.to_string());
    let amend_backup = out["backup"].as_str().unwrap().to_owned();
    let index = index_and_files(&repo, &["a.txt", "b.txt", "c.txt"]);

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), old);
    assert_eq!(index_and_files(&repo, &["a.txt", "b.txt", "c.txt"]), index);
    // What `amend` had added goes back to staged.
    assert_eq!(porcelain(&repo), "A  c.txt");
    assert!(
        out["undone"]["description"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Amend ")
    );
    assert_eq!(out["undone"]["backup"], amend_backup);
    // The `amend` commit is now saved; the rewritten commit was already in the `amend` backup.
    let backup = out["backups"][0].as_str().unwrap();
    assert_eq!(repo.rev(backup), amended);
    assert_eq!(repo.rev(&amend_backup), old);

    let out = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), amended);
    assert_eq!(
        out["backups"],
        json!([]),
        "the rewritten commit already has a backup"
    );
    assert_eq!(porcelain(&repo), "");
}

#[test]
fn undo_with_detached_head() {
    let h = repo_detached_head();
    let repo = &h.repo;
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let branches = repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]);
    let (r, mut s) = open(repo);
    let out = run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "detached" }),
    );
    assert_eq!(out["branch"], Value::Null);
    let commit = repo.rev("HEAD");

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["undone"]["refs"], json!([]));
    assert_eq!(
        out["undone"]["head"],
        json!({ "before": { "detached": h.f1.to_string() }, "after": { "detached": commit.to_string() } })
    );
    assert!(
        !repo
            .git_output(&["symbolic-ref", "-q", "HEAD"])
            .status
            .success(),
        "HEAD stays detached"
    );
    assert_eq!(repo.rev("HEAD"), h.f1);
    assert_eq!(porcelain(repo), "A  c.txt");
    let backup = out["backups"][0].as_str().unwrap().to_owned();
    assert_eq!(repo.rev(&backup), commit);
    let only_backup_added = repo
        .git(&["for-each-ref", "--format=%(refname) %(objectname)"])
        .lines()
        .filter(|l| !l.starts_with(&backup))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(only_backup_added, branches, "no branch moved");
    assert_eq!(
        repo.git(&["log", "-g", "-1", "--format=%gs", "HEAD"]),
        format!(
            "project: undo Commit {} “detached”",
            &commit.to_string()[..7]
        )
    );

    // HEAD moved from outside: the redo is refused.
    repo.detach(h.c1);
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(repo.rev("HEAD"), h.c1);
    repo.detach(h.f1);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("HEAD"), commit);
}

#[test]
fn several_undos_then_a_new_commit_clears_redo() {
    let repo = Repo::new();
    let start = repo.commit_file("a.txt", "0\n", "start");
    let (r, mut s) = open(&repo);
    let mut ids = Vec::new();
    for n in 1..=3 {
        repo.write("a.txt", &format!("{n}\n"));
        run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
        run(
            &r,
            &mut s,
            "commit.create",
            json!({ "message": format!("commit {n}") }),
        );
        ids.push(repo.rev("HEAD"));
    }
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"].as_array().unwrap().len(), 3);

    for expected in [ids[1], ids[0], start] {
        run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(repo.rev("refs/heads/main"), expected);
    }
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    // The worktree and the index are always those of the last commit.
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "3\n"
    );
    assert_eq!(porcelain(&repo), "M  a.txt");

    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), ids[0]);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["undone"].as_array().unwrap().len(), 2);

    // A new operation empties the redo.
    run(&r, &mut s, "commit.create", json!({ "message": "new" }));
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["can_redo"], false);
    assert_eq!(history["undone"], json!([]));
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
}

#[test]
fn the_undone_commit_survives_gc() {
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "Add b" }));
    let commit = repo.rev("HEAD").to_string();
    let out = run(&r, &mut s, "edit.undo", json!({}));
    let backup = out["backups"][0].as_str().unwrap().to_owned();

    // The expired reflog no longer keeps it: only the backup ref.
    repo.git(&["reflog", "expire", "--expire=now", "--all"]);
    repo.git(&["gc", "--quiet", "--prune=now"]);
    repo.git(&["cat-file", "-e", &format!("{commit}^{{commit}}")]);
    // The redo works after the gc too.
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main").to_string(), commit);

    // Counter-check: without the backup (and with the branch behind) the gc deletes it.
    run(&r, &mut s, "edit.undo", json!({}));
    repo.git(&["update-ref", "-d", &backup]);
    repo.git(&["reflog", "expire", "--expire=now", "--all"]);
    repo.git(&["gc", "--quiet", "--prune=now"]);
    assert!(
        !repo
            .git_output(&["cat-file", "-e", &commit])
            .status
            .success()
    );
}

#[test]
fn a_hook_that_rejects_the_transaction_changes_nothing() {
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "Add b" }));
    let commit = repo.rev("HEAD");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = repo.path().join(".git/hooks/reference-transaction");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(
            &hook,
            "#!/bin/sh\nif [ \"$1\" = prepared ] && grep -q refs/heads/main; then\n  echo 'no: rejected' >&2\n  exit 1\nfi\n",
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::Git { .. }), "{err}");
        assert!(err.to_string().contains("rejected"), "{err}");
        assert_eq!(repo.rev("refs/heads/main"), commit);
        std::fs::remove_file(&hook).unwrap();
    }
    // The entry is still there: without the hook the undo succeeds.
    run(&r, &mut s, "edit.undo", json!({}));
    assert_ne!(repo.rev("refs/heads/main"), commit);
}

#[test]
fn hostile_config_does_not_change_the_result() {
    let repo = repo_ready_to_commit();
    // Repo configuration that could interfere: no reflog, colors, pager, aliases.
    repo.git(&["config", "core.logAllRefUpdates", "false"]);
    repo.git(&["config", "color.ui", "always"]);
    repo.git(&["config", "core.pager", "false"]);
    repo.git(&["config", "alias.update-ref", "!false"]);
    let before = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "Add b" }));
    let commit = repo.rev("HEAD");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), before);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), commit);
}

#[test]
fn refused_while_a_merge_is_in_progress() {
    let (repo, _) = repo_with_divergent_branches();
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "c" }));
    repo.conflict("other");
    let before = refs(&repo);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("merge"), "{err}");
    assert_eq!(refs(&repo), before);
}

#[test]
fn a_symbolic_ref_to_the_moved_branch_does_not_count_as_protection() {
    let repo = repo_ready_to_commit();
    // `master` is an alias of `main`: `for-each-ref --contains` shows it with the value of
    // `main` before the transaction, but afterwards it keeps nothing.
    repo.git(&["symbolic-ref", "refs/heads/master", "refs/heads/main"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "Add b" }));
    let commit = repo.rev("HEAD").to_string();
    let out = run(&r, &mut s, "edit.undo", json!({}));
    let backup = out["backups"][0]
        .as_str()
        .expect("backup created")
        .to_owned();
    assert_eq!(repo.rev(&backup).to_string(), commit);

    repo.git(&["reflog", "expire", "--expire=now", "--all"]);
    repo.git(&["gc", "--quiet", "--prune=now"]);
    repo.git(&["cat-file", "-e", &format!("{commit}^{{commit}}")]);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main").to_string(), commit);
    assert_eq!(repo.rev("refs/heads/master").to_string(), commit);
}

#[test]
fn a_failed_backup_moves_nothing() {
    let repo = repo_ready_to_commit();
    let before = repo.rev("HEAD").to_string();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "Add b" }));
    let commit = repo.rev("HEAD");
    // A ref `refs/project/backup` (a file) takes the name of the backups directory: the
    // backup ref cannot be created. It points to the previous commit, so it does not protect the
    // one to undo.
    repo.git(&["update-ref", "refs/project/backup", &before]);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::Git { command, .. } if command == "update-ref"),
        "{err}"
    );
    assert_eq!(repo.rev("refs/heads/main"), commit, "nothing moved");
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["can_undo"], true, "the entry stays");
    // With the obstacle removed, the undo succeeds.
    repo.git(&["update-ref", "-d", "refs/project/backup"]);
    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["backups"].as_array().unwrap().len(), 1);
    assert_eq!(repo.rev("refs/heads/main").to_string(), before);
}

#[cfg(unix)]
#[test]
fn head_changed_during_the_transaction_is_reported() {
    use std::os::unix::fs::PermissionsExt;
    let h = repo_detached_head();
    let repo = &h.repo;
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let (r, mut s) = open(repo);
    run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "detached" }),
    );
    // Another process puts HEAD back on `main` right after our transaction (simulated
    // with a `reference-transaction` hook in the `committed` state, only once).
    let hook = repo.path().join(".git/hooks/reference-transaction");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        "#!/bin/sh\nd=$(git rev-parse --absolute-git-dir) || exit 1\nif [ \"$1\" = committed ] && grep -q ' HEAD$' && [ ! -e \"$d/done\" ]; then\n  touch \"$d/done\"\n  git symbolic-ref HEAD refs/heads/main\nfi\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = run(&r, &mut s, "edit.undo", json!({}));
    let warnings = out["warnings"].as_array().unwrap();
    assert!(!warnings.is_empty(), "{out}");
    assert!(
        warnings[0]["text"].as_str().unwrap().contains("HEAD"),
        "{warnings:?}"
    );
    assert_eq!(repo.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    // No branch was touched.
    assert_eq!(repo.rev("refs/heads/main"), h.m);
}

#[test]
fn normal_undo_has_no_warnings() {
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "Add b" }));
    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["warnings"], json!([]));
    let out = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(out["warnings"], json!([]));
}

#[test]
fn refused_during_git_am_and_a_stopped_sequencer() {
    // `git am` stopped on a patch that does not apply.
    let repo = Repo::new();
    repo.commit_file("a.txt", "one\n", "base");
    repo.branch("other");
    repo.checkout("other");
    repo.commit_file("a.txt", "two\n", "on other");
    let patch = repo.git(&["format-patch", "-1", "--stdout", "HEAD"]);
    repo.checkout("main");
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "c" }));
    repo.commit_file("a.txt", "three\n", "conflict");
    let patch_file = repo.path().join("p.patch");
    std::fs::write(&patch_file, format!("{patch}\n")).unwrap();
    repo.git_fails(&["am", "--quiet", patch_file.to_str().unwrap()]);
    assert!(repo.path().join(".git/rebase-apply/applying").exists());
    let status = run(&r, &mut s, "repo.status", json!({}));
    assert_eq!(status["operation"], "Am");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("git am"), "{err}");
    repo.write("d.txt", "d\n");
    repo.stage(&["d.txt"]);
    let err = r
        .run("commit.create", &mut s, json!({ "message": "x" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    // Cherry-pick of two commits: the first conflicts, resolved with `git commit`; the
    // sequencer stays stopped without `CHERRY_PICK_HEAD`.
    let repo = Repo::new();
    repo.commit_file("a.txt", "one\n", "base");
    repo.branch("other");
    repo.checkout("other");
    let p1 = repo.commit_file("a.txt", "two\n", "one on other");
    let p2 = repo.commit_file("b.txt", "b\n", "two on other");
    repo.checkout("main");
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "c" }));
    repo.commit_file("a.txt", "three\n", "conflict");
    repo.git_fails(&["cherry-pick", &p1.to_string(), &p2.to_string()]);
    repo.write("a.txt", "resolved\n");
    repo.stage(&["a.txt"]);
    repo.git(&["commit", "--quiet", "--no-edit"]);
    assert!(!repo.path().join(".git/CHERRY_PICK_HEAD").exists());
    assert!(repo.path().join(".git/sequencer").exists());
    let status = run(&r, &mut s, "repo.status", json!({}));
    assert_eq!(status["operation"], "CherryPick");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("cherry-pick"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_checkout_just_before_the_transaction_is_reported() {
    use std::os::unix::fs::PermissionsExt;
    let h = repo_detached_head();
    let repo = &h.repo;
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let (r, mut s) = open(repo);
    run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "detached" }),
    );
    let commit = repo.rev("HEAD");
    let branches = repo.git(&[
        "for-each-ref",
        "--format=%(refname) %(objectname)",
        "refs/heads/",
    ]);
    // Another process does a checkout between the check and the transaction: simulated by a hook
    // on the creation of the backup ref (which comes right before), with a branch `other` on the
    // expected commit. The transaction (`option no-deref` + `update HEAD`) succeeds and detaches
    // HEAD again: the final HEAD is the requested one, but the HEAD reflog has two new entries.
    let hook = repo.path().join(".git/hooks/reference-transaction");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nd=$(git rev-parse --absolute-git-dir) || exit 1\n\
             if [ \"$1\" = committed ] && grep -q ' refs/project/backup/undo-' && [ ! -e \"$d/done\" ]; then\n\
             \x20 touch \"$d/done\"\n\
             \x20 git branch other {commit}\n\
             \x20 git symbolic-ref -m 'checkout: moving to other' HEAD refs/heads/other\n\
             fi\nexit 0\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = run(&r, &mut s, "edit.undo", json!({}));
    let warnings = out["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{out}");
    assert!(
        warnings[0]["text"]
            .as_str()
            .unwrap()
            .contains("new entries"),
        "{warnings:?}"
    );
    // HEAD detached on the previous commit, as requested; the branches do not move (there is only
    // `other`, created by the "checkout").
    assert!(
        !repo
            .git_output(&["symbolic-ref", "-q", "HEAD"])
            .status
            .success()
    );
    assert_eq!(repo.rev("HEAD"), h.f1);
    assert_eq!(repo.rev("refs/heads/other"), commit);
    let now = repo.git(&[
        "for-each-ref",
        "--format=%(refname) %(objectname)",
        "refs/heads/",
    ]);
    let without_other: Vec<&str> = now
        .lines()
        .filter(|l| !l.starts_with("refs/heads/other "))
        .collect();
    assert_eq!(without_other.join("\n"), branches);
}
