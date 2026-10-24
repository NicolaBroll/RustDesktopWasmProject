//! Integration tests of `remote.pull` (S26) with the `CliBackend` and a local remote (`git
//! init --bare`) to which a second clone publishes: fast-forward, merge, rebase according to
//! the configuration, conflicts (the merge stays in progress, the rebase is aborted), local
//! changes, branches without upstream, undo and redo, non-checked-out branch, names starting
//! with `-`.

mod common;

use common::Repo;
use project_domain::Oid;
use project_engine::{EngineError, PullOperation, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn open(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

fn pull(r: &Registry, s: &mut Session, params: Value) -> Value {
    r.run("remote.pull", s, params)
        .unwrap_or_else(|e| panic!("remote.pull: {e}"))
}

/// The bare remote, a clone that publishes to it (`other`) and the repo under test (`work`):
/// both with `main` on `c1` (`a.txt`), which follows `origin/main`.
struct Setup {
    _remote: Repo,
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
        _remote: remote,
        other,
        work,
        c1,
    }
}

/// `other` publishes a commit on `main` that writes `file`.
fn publish(other: &Repo, file: &str, content: &str, message: &str) -> Oid {
    let id = other.commit_file(file, content, message);
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    id
}

fn read(repo: &Repo, file: &str) -> Option<String> {
    std::fs::read_to_string(repo.path().join(file)).ok()
}

fn backups(repo: &Repo) -> Vec<String> {
    let out = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/project/backup/",
    ]);
    out.lines().map(str::to_owned).collect()
}

fn clean(repo: &Repo) -> bool {
    repo.git(&["status", "--porcelain"]).is_empty()
}

#[test]
fn fast_forward_then_undo_and_redo() {
    let Setup {
        other,
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    let c2 = publish(&other, "b.txt", "bi\n", "second");
    let (r, mut s) = open(&work);

    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(out["strategy_source"], "default");
    assert_eq!(out["branch"], "main");
    assert_eq!(out["remote"], "origin");
    assert_eq!(out["upstream"], "refs/remotes/origin/main");
    assert_eq!(out["before"], c1.to_string());
    assert_eq!(out["after"], c2.to_string());
    assert_eq!(
        (out["ahead"].as_u64(), out["behind"].as_u64()),
        (Some(0), Some(1))
    );
    assert_eq!(
        out["fetch"]["updated"],
        json!([{ "name": "refs/remotes/origin/main", "before": c1.to_string(), "after": c2.to_string() }])
    );
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(backup.starts_with("refs/project/backup/pull-"), "{backup}");
    assert_eq!(work.rev(&backup), c1);
    assert_eq!(work.rev("HEAD"), c2);
    assert_eq!(read(&work, "b.txt").as_deref(), Some("bi\n"));
    assert!(clean(&work));
    assert_eq!(out["can_undo"], true);
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Pull of `main` (fast-forward")
    );
    assert_eq!(out["status"]["head"]["Branch"]["target"], c2.to_string());

    // Undo: branch, index and worktree go back to `c1`; unrelated changes stay.
    work.write("a.txt", "my change\n");
    let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(undo["undone"]["command"], "remote.pull");
    assert_eq!(work.rev("HEAD"), c1);
    assert_eq!(work.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(read(&work, "b.txt"), None);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("my change\n"));
    // `c2` stays on `origin/main`: no extra backup.
    assert_eq!(undo["backups"], json!([]));

    let redo = r.run("edit.redo", &mut s, json!({})).unwrap();
    assert_eq!(redo["redone"]["command"], "remote.pull");
    assert_eq!(work.rev("HEAD"), c2);
    assert_eq!(read(&work, "b.txt").as_deref(), Some("bi\n"));
    assert_eq!(read(&work, "a.txt").as_deref(), Some("my change\n"));

    // Already up to date: nothing to do, nothing in the journal.
    let out = pull(&r, &mut s, json!({ "remote": "origin" }));
    assert_eq!(out["strategy"], "up_to_date");
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["before"], out["after"]);
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["done"].as_array().unwrap().len(), 1);
}

#[test]
fn undo_of_a_pull_never_overwrites_local_changes() {
    let Setup {
        other,
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    let c2 = publish(&other, "a.txt", "two\n", "second");
    let (r, mut s) = open(&work);
    pull(&r, &mut s, json!({}));
    assert_eq!(work.rev("HEAD"), c2);

    // `a.txt` was changed by the pull and now has a local change: the undo would lose it.
    work.write("a.txt", "mine\n");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(work.rev("HEAD"), c2);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("mine\n"));
    assert_eq!(
        r.run("edit.history", &mut s, json!({})).unwrap()["can_undo"],
        true
    );

    // A file that the undo would recreate and that now exists as ignored: refused too.
    work.git(&["checkout", "--quiet", "--", "a.txt"]);
    other.git(&["rm", "--quiet", "a.txt"]);
    other.commit("remove a");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    pull(&r, &mut s, json!({}));
    assert!(read(&work, "a.txt").is_none());
    work.write(".git/info/exclude", "a.txt\n");
    work.write("a.txt", "ignored but mine\n");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(read(&work, "a.txt").as_deref(), Some("ignored but mine\n"));
    std::fs::remove_file(work.path().join("a.txt")).unwrap();
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), c2);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("two\n"));

    // The branch moved from outside: `UndoConflict`, nothing changes.
    let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(undo["undone"]["command"], "remote.pull");
    assert_eq!(work.rev("HEAD"), c1);
    r.run("edit.redo", &mut s, json!({})).unwrap();
    work.commit_file("c.txt", "c\n", "from terminal");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
}

#[test]
fn diverged_branches_merge_by_default() {
    let Setup {
        other,
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    let c2 = publish(&other, "b.txt", "bi\n", "second");
    let mine = work.commit_file("c.txt", "ci\n", "mine");
    // Merge options from the configuration that would change the result: neutralized.
    work.git(&["config", "branch.main.mergeOptions", "--squash --no-commit"]);
    work.git(&["config", "merge.ff", "only"]);
    let (r, mut s) = open(&work);

    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "merge");
    assert_eq!(out["strategy_source"], "default");
    let merge = work.rev("HEAD");
    assert_eq!(out["after"], merge.to_string());
    assert_eq!(
        work.git(&["log", "-1", "--format=%P"]),
        format!("{mine} {c2}")
    );
    assert_eq!(
        work.git(&["log", "-1", "--format=%s"]),
        "Merge remote-tracking branch 'origin/main'"
    );
    assert!(clean(&work));
    assert_eq!(read(&work, "b.txt").as_deref(), Some("bi\n"));

    // Undo: the merge commit goes into a backup, the branch goes back to `mine`.
    let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), mine);
    let saved = undo["backups"].as_array().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(work.rev(saved[0].as_str().unwrap()), merge);
    assert!(clean(&work));
    assert_eq!(read(&work, "b.txt"), None);
    let _ = c1;

    // `pull.ff=false`: a merge commit even when a fast-forward would be possible.
    let (r, mut s) = open(&work);
    work.git(&["reset", "--quiet", "--hard", "origin/main~1"]);
    work.git(&["config", "pull.ff", "false"]);
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "merge");
    assert_eq!(out["strategy_source"], "config");
    assert_eq!(
        work.git(&["log", "-1", "--format=%P"]).split(' ').count(),
        2
    );

    // `pull.ff=only` with diverged branches: refused, the branch is not touched.
    work.git(&["reset", "--quiet", "--hard", &mine.to_string()]);
    work.git(&["config", "pull.ff", "only"]);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(work.rev("HEAD"), mine);
    // `strategy` overrides the configuration.
    let out = pull(&r, &mut s, json!({ "strategy": "rebase" }));
    assert_eq!(out["strategy"], "rebase");
    assert_eq!(out["strategy_source"], "param");
}

#[test]
fn rebase_from_the_configuration() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    let c2 = publish(&other, "b.txt", "bi\n", "second");
    let mine = work.commit_file("c.txt", "ci\n", "mine");
    work.git(&["config", "pull.rebase", "true"]);
    let (r, mut s) = open(&work);

    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "rebase");
    assert_eq!(out["strategy_source"], "config");
    let rebased = work.rev("HEAD");
    assert_ne!(rebased, mine);
    assert_eq!(work.git(&["log", "-1", "--format=%P"]), c2.to_string());
    assert_eq!(work.git(&["log", "-1", "--format=%s"]), "mine");
    assert!(clean(&work));
    // The previous commit is in the pull backup.
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(work.rev(backup), mine);
    assert!(next_undo(&r, &mut s).contains("rebase onto origin/main"));

    // Undo: goes back to the original commit (kept by the pull backup, no extra backup for
    // it; the rewritten commit gets one).
    let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), mine);
    let saved = undo["backups"].as_array().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(work.rev(saved[0].as_str().unwrap()), rebased);
    r.run("edit.redo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), rebased);

    // `branch.<name>.rebase` wins over `pull.rebase`.
    work.git(&["reset", "--quiet", "--hard", &mine.to_string()]);
    work.git(&["config", "branch.main.rebase", "false"]);
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "merge");
    // `interactive`: refused before the fetch.
    work.git(&["reset", "--quiet", "--hard", &mine.to_string()]);
    work.git(&["config", "branch.main.rebase", "interactive"]);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(work.rev("HEAD"), mine);
}

fn next_undo(r: &Registry, s: &mut Session) -> String {
    r.run("edit.history", s, json!({})).unwrap()["next_undo"]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn a_merge_conflict_stays_in_progress_then_continue() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    let theirs = publish(&other, "a.txt", "theirs\n", "theirs");
    let mine = work.commit_file("a.txt", "ours\n", "ours");
    // Any untracked file does not block the pull and stays as it is.
    work.write("notes.txt", "?\n");
    let (r, mut s) = open(&work);
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "merge");
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert!(
        out["message"].as_str().unwrap().contains("CONFLICT"),
        "{out}"
    );
    assert_eq!(out["before"], mine.to_string());
    assert_eq!(out["after"], mine.to_string());
    assert_eq!(out["warnings"], json!([]));
    // The backup stays, nothing in the journal, the fetch is done.
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(backup.starts_with("refs/project/backup/pull-"), "{backup}");
    assert_eq!(backups(&work), vec![backup.clone()]);
    assert_eq!(work.rev(&backup), mine);
    assert_eq!(out["can_undo"], false);
    assert_eq!(work.rev("origin/main"), theirs);
    // The merge is in progress: MERGE_HEAD is the upstream, the message is the pull one.
    assert_eq!(work.rev("HEAD"), mine);
    assert_eq!(work.rev("MERGE_HEAD"), theirs);
    let merge_msg = std::fs::read_to_string(work.path().join(".git/MERGE_MSG")).unwrap();
    assert!(
        merge_msg.starts_with("Merge remote-tracking branch 'origin/main'\n"),
        "{merge_msg}"
    );
    assert_eq!(out["status"]["operation"], "Merge");
    let op = r.run("repo.operation", &mut s, json!({})).unwrap();
    assert_eq!(op["operation"], "Merge");
    assert_eq!(op["conflicts"], json!(["a.txt"]));
    assert_eq!(op["incoming"][0]["label"], "origin/main");
    assert_eq!(op["continue"], "merge.continue");
    assert_eq!(op["message"], "Merge remote-tracking branch 'origin/main'");
    // Another pull is refused while the merge is in progress; so is the undo.
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(r.run("edit.undo", &mut s, json!({})).is_err());

    // Resolved and staged, `merge.continue` makes the merge commit and goes into the journal.
    work.write("a.txt", "resolved\n");
    r.run("index.stage", &mut s, json!({ "paths": ["a.txt"] }))
        .unwrap();
    let done = r.run("merge.continue", &mut s, json!({})).unwrap();
    assert_eq!(
        done["message"],
        "Merge remote-tracking branch 'origin/main'"
    );
    let merge = work.rev("HEAD");
    assert_eq!(work.rev("HEAD^1"), mine);
    assert_eq!(work.rev("HEAD^2"), theirs);
    assert_eq!(
        work.git(&["log", "-1", "--format=%B"]).trim(),
        "Merge remote-tracking branch 'origin/main'"
    );
    assert!(!work.path().join(".git/MERGE_HEAD").exists());
    assert_eq!(done["can_undo"], true);
    assert!(
        next_undo(&r, &mut s).contains("origin/main"),
        "{}",
        next_undo(&r, &mut s)
    );
    assert_eq!(read(&work, "notes.txt").as_deref(), Some("?\n"));
    // The undo brings the branch back to before the pull, with index and worktree.
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), mine);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("ours\n"));
    r.run("edit.redo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), merge);
}

#[test]
fn a_merge_conflict_then_abort_is_back_to_before() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "a.txt", "theirs\n", "theirs");
    publish(&other, "b.txt", "new\n", "b from remote");
    let mine = work.commit_file("a.txt", "ours\n", "ours");
    let (r, mut s) = open(&work);
    let out = pull(&r, &mut s, json!({ "strategy": "merge" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(read(&work, "b.txt").as_deref(), Some("new\n"));
    let backup = out["backup"].as_str().unwrap().to_owned();

    // Without confirmation nothing changes.
    let err = r.run("merge.abort", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(work.path().join(".git/MERGE_HEAD").exists());
    let aborted = r
        .run("merge.abort", &mut s, json!({ "confirm": true }))
        .unwrap();
    assert!(
        aborted["backup"]
            .as_str()
            .unwrap()
            .starts_with("refs/project/backup/merge-abort-"),
        "{aborted}"
    );
    // Everything as it was before the pull: the worktree was clean.
    assert_eq!(work.rev("HEAD"), mine);
    assert!(clean(&work), "{}", work.git(&["status", "--porcelain"]));
    assert_eq!(read(&work, "a.txt").as_deref(), Some("ours\n"));
    assert_eq!(read(&work, "b.txt"), None);
    assert!(!work.path().join(".git/MERGE_HEAD").exists());
    assert_eq!(aborted["can_undo"], false);
    // The pull backup stays (it is the branch as it was).
    assert!(backups(&work).contains(&backup), "{:?}", backups(&work));
    // The pull can be repeated.
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["in_progress"], true);
}

#[test]
fn a_rebase_conflict_is_still_aborted_and_reported() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "a.txt", "theirs\n", "theirs");
    let mine = work.commit_file("a.txt", "ours\n", "ours");
    work.git(&["config", "pull.rebase", "true"]);
    let (r, mut s) = open(&work);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    let EngineError::PullConflict {
        branch,
        upstream,
        operation,
        files,
        message,
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(branch, "main");
    assert_eq!(upstream, "origin/main");
    assert_eq!(*operation, PullOperation::Rebase);
    assert_eq!(files, &["a.txt"]);
    assert!(message.contains("CONFLICT"), "{message}");
    // Aborted: everything as it was, no operation in progress, no backup left.
    assert_eq!(work.rev("HEAD"), mine);
    assert!(clean(&work), "{}", work.git(&["status", "--porcelain"]));
    assert_eq!(read(&work, "a.txt").as_deref(), Some("ours\n"));
    assert!(!work.path().join(".git/MERGE_HEAD").exists());
    assert!(!work.path().join(".git/rebase-merge").exists());
    assert_eq!(backups(&work), Vec::<String>::new());
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["can_undo"], false);
    // The fetch stays done.
    assert_ne!(work.rev("origin/main"), work.rev("HEAD~1"));
}

/// Installs the hook `name` of `repo` with the script `body` (after `#!/bin/sh`).
fn install_hook(repo: &Repo, name: &str, body: &str) -> std::path::PathBuf {
    let hook = repo.path().join(".git/hooks").join(name);
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, format!("#!/bin/sh\n{body}")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    hook
}

#[test]
fn a_failed_rebase_abort_keeps_the_backup() {
    // The `post-checkout` hook (the rebase runs it after checking out the upstream) leaves
    // `index.lock`: the pick stops and `rebase --abort` fails too, the rebase stays in progress.
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "b.txt", "b\n", "from remote");
    let mine = work.commit_file("c.txt", "c\n", "mine");
    install_hook(
        &work,
        "post-checkout",
        ": > \"$(git rev-parse --git-dir)/index.lock\"\n",
    );
    let (r, mut s) = open(&work);
    let err = r
        .run("remote.pull", &mut s, json!({ "strategy": "rebase" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::GitWithReason { .. }), "{err}");
    let text = err.to_string();
    assert!(text.contains("--abort` failed"), "{text}");
    let kept = backups(&work);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].starts_with("refs/project/backup/pull-"), "{kept:?}");
    assert!(
        text.contains(&kept[0]),
        "the error names the backup: {text}"
    );
    assert_eq!(work.rev(&kept[0]), mine);
    assert!(work.path().join(".git/rebase-merge").exists());
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["can_undo"], false);
    // Cleanup from the terminal: with the lock removed, the rebase aborts and the branch is as it was.
    std::fs::remove_file(work.path().join(".git/index.lock")).unwrap();
    std::fs::remove_file(work.path().join(".git/hooks/post-checkout")).unwrap();
    work.git(&["rebase", "--abort"]);
    assert_eq!(work.rev("HEAD"), mine);
}

#[cfg(unix)]
#[test]
fn a_non_utf8_conflict_is_listed_once() {
    use std::os::unix::ffi::OsStrExt;
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    let name = std::ffi::OsStr::from_bytes(b"caf\xe9.txt");
    std::fs::write(other.path().join(name), "theirs\n").unwrap();
    other.git(&["add", "-A"]);
    other.commit("theirs");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    std::fs::write(work.path().join(name), "ours\n").unwrap();
    work.git(&["add", "-A"]);
    work.commit("ours");
    let (r, mut s) = open(&work);
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["in_progress"], true);
    assert_eq!(
        out["conflicts"].as_array().unwrap().len(),
        1,
        "{}",
        out["conflicts"]
    );
}

#[test]
fn a_merge_stopped_by_a_hook_stays_in_progress() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "b.txt", "b\n", "from remote");
    let mine = work.commit_file("c.txt", "c\n", "mine");
    let hook = install_hook(&work, "pre-merge-commit", "echo no >&2\nexit 1\n");
    let (r, mut s) = open(&work);
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!([]));
    assert_eq!(work.rev("HEAD"), mine);
    assert!(work.path().join(".git/MERGE_HEAD").exists());
    assert_eq!(out["can_undo"], false);
    let op = r.run("repo.operation", &mut s, json!({})).unwrap();
    assert_eq!(op["ready"], true);
    // With the hook removed, `merge.continue` finishes (`git commit` does not use `pre-merge-commit`).
    std::fs::remove_file(&hook).unwrap();
    r.run("merge.continue", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD^1"), mine);
    assert_eq!(read(&work, "b.txt").as_deref(), Some("b\n"));
}

#[test]
fn local_changes_block_the_pull() {
    let Setup {
        other,
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    let c2 = publish(&other, "a.txt", "two\n", "second");
    let (r, mut s) = open(&work);

    // Fast-forward that would overwrite a local change: git refuses, nothing changes.
    work.write("a.txt", "mine\n");
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    assert_eq!(work.rev("HEAD"), c1);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("mine\n"));
    assert_eq!(work.rev("origin/main"), c2, "the fetch is done");
    assert_eq!(backups(&work), Vec::<String>::new());

    // An untracked file that the pull would create: refused, the file stays.
    work.git(&["checkout", "--quiet", "--", "a.txt"]);
    publish(&other, "new.txt", "theirs\n", "new");
    work.write("new.txt", "mine\n");
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(read(&work, "new.txt").as_deref(), Some("mine\n"));
    assert_eq!(work.rev("HEAD"), c1);
    std::fs::remove_file(work.path().join("new.txt")).unwrap();

    // A merge with any change to tracked files: refused before writing.
    work.commit_file("c.txt", "c\n", "mine");
    let mine = work.rev("HEAD");
    work.write("c.txt", "change\n");
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("autostash"), "{err}");
    assert_eq!(work.rev("HEAD"), mine);
    assert_eq!(read(&work, "c.txt").as_deref(), Some("change\n"));
    // Any untracked file, instead, does not block.
    work.git(&["checkout", "--quiet", "--", "c.txt"]);
    work.write("notes.txt", "?\n");
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "merge");
    assert_eq!(read(&work, "notes.txt").as_deref(), Some("?\n"));
}

#[test]
fn no_upstream_detached_head_and_operations_in_progress() {
    let Setup {
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    let (r, mut s) = open(&work);
    work.branch("only");
    work.checkout("only");
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let text = err.to_string();
    assert!(text.contains("--set-upstream-to"), "{text}");
    assert!(text.contains("remote.push"), "{text}");

    // `remote` different from the upstream's one.
    work.checkout("main");
    let err = r
        .run("remote.pull", &mut s, json!({ "remote": "elsewhere" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    work.detach(c1);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("detached"), "{err}");

    // A merge in progress.
    work.checkout("main");
    let (theirs, _) = (work.diverge("side", "a.txt", "x\n", "y\n"), ());
    work.conflict("side");
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let _ = theirs;

    for params in [
        json!({ "strategy": "octopus" }),
        json!({ "force": true }),
        json!({ "branch": "" }),
        json!({ "remote": "a b" }),
    ] {
        let err = r.run("remote.pull", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
}

#[test]
fn a_branch_that_is_not_checked_out_only_fast_forwards() {
    let Setup {
        other,
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    // A `-f` branch on the remote and locally (created with `update-ref`: git no longer accepts
    // such a name for a new branch), which follows `origin/-f`.
    other.branch("feat");
    other.checkout("feat");
    other.commit_file("f.txt", "f\n", "on -f");
    other.git(&["push", "--quiet", "origin", "refs/heads/feat:refs/heads/-f"]);
    work.git(&["update-ref", "refs/heads/-f", &c1.to_string()]);
    work.git(&["config", "branch.-f.remote", "origin"]);
    work.git(&["config", "branch.-f.merge", "refs/heads/-f"]);
    let f2 = other.commit_file("f.txt", "ff\n", "again on -f");
    other.git(&["push", "--quiet", "origin", "refs/heads/feat:refs/heads/-f"]);
    let (r, mut s) = open(&work);

    let out = pull(&r, &mut s, json!({ "branch": "-f" }));
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(work.rev("refs/heads/-f"), f2);
    // HEAD and worktree are not touched.
    assert_eq!(work.rev("HEAD"), c1);
    assert!(read(&work, "f.txt").is_none());
    assert!(clean(&work));
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("refs/heads/-f"), c1);

    // Diverged: only from the branch itself.
    work.git(&[
        "update-ref",
        "refs/heads/-f",
        &work.commit_file("g.txt", "g\n", "g").to_string(),
    ]);
    let err = r
        .run("remote.pull", &mut s, json!({ "branch": "-f" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("branch.checkout"), "{err}");
}

#[test]
fn cancel_and_unreachable_remote() {
    let Setup {
        other,
        work,
        c1,
        _remote: remote,
    } = setup();
    publish(&other, "b.txt", "b\n", "second");
    let (r, mut s) = open(&work);
    let token = project_engine::CancelToken::new();
    token.cancel();
    let err = r
        .run_with(
            "remote.pull",
            &mut s,
            json!({}),
            project_engine::JobContext::new().with_cancel(token),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Cancelled {
                command: "remote.pull"
            }
        ),
        "{err}"
    );
    assert_eq!(work.rev("origin/main"), c1, "nothing downloaded");
    let missing = work.path().join("does-not-exist.git");
    work.git(&["remote", "set-url", "origin", missing.to_str().unwrap()]);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(err, EngineError::RemoteUnreachable { .. }),
        "{err}"
    );
    assert_eq!(work.rev("HEAD"), c1);
    drop(remote);
}

/// `work` ignores `*.env` and has its own `secret.env`; the remote adds a tracked
/// `secret.env` with different content.
fn ignored_secret(other: &Repo, work: &Repo) {
    work.write(".git/info/exclude", "*.env\n");
    work.write("secret.env", "my key\n");
    other.write("secret.env", "the remote one\n");
    other.stage(&["secret.env"]);
    other.commit("add secret.env");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
}

#[test]
fn ignored_files_are_never_overwritten() {
    // Fast-forward, rebase and a merge that would end in conflict on another file: refused
    // before writing, the ignored file stays identical.
    for case in ["ff", "rebase", "merge_conflict"] {
        let Setup {
            other,
            work,
            _remote: _keep,
            ..
        } = setup();
        match case {
            "rebase" => {
                work.commit_file("c.txt", "c\n", "mine");
                work.git(&["config", "pull.rebase", "true"]);
            }
            "merge_conflict" => {
                publish(&other, "a.txt", "theirs\n", "theirs");
                work.commit_file("a.txt", "ours\n", "ours");
            }
            _ => {}
        }
        ignored_secret(&other, &work);
        let head = work.rev("HEAD");
        let (r, mut s) = open(&work);
        let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{case}: {err}");
        assert!(err.to_string().contains("secret.env"), "{case}: {err}");
        assert_eq!(
            read(&work, "secret.env").as_deref(),
            Some("my key\n"),
            "{case}"
        );
        assert_eq!(work.rev("HEAD"), head, "{case}");
        assert!(clean(&work), "{case}");
        assert_eq!(backups(&work), Vec::<String>::new(), "{case}");

        // With the file moved away, the pull succeeds (except the conflict, which stays in progress).
        std::fs::remove_file(work.path().join("secret.env")).unwrap();
        let out = pull(&r, &mut s, json!({}));
        assert_eq!(
            out["in_progress"],
            case == "merge_conflict",
            "{case}: {out}"
        );
        assert_eq!(
            read(&work, "secret.env").as_deref(),
            Some("the remote one\n"),
            "{case}"
        );
    }
}

#[test]
fn a_conflict_resolved_by_rerere_is_still_a_conflict() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "a.txt", "theirs\n", "theirs");
    let mine = work.commit_file("a.txt", "ours\n", "ours");
    work.git(&["config", "rerere.enabled", "true"]);
    work.git(&["config", "rerere.autoUpdate", "true"]);
    // A resolution recorded by rerere for the same conflict.
    work.git(&["fetch", "--quiet", "origin"]);
    work.git_fails(&["merge", "--no-edit", "origin/main"]);
    work.write("a.txt", "resolved\n");
    work.stage(&["a.txt"]);
    work.git(&["commit", "--quiet", "--no-edit"]);
    work.git(&["reset", "--quiet", "--hard", &mine.to_string()]);

    let (r, mut s) = open(&work);
    let out = pull(&r, &mut s, json!({}));
    // The merge stays stopped and the file counts as a conflict even if rerere already resolved
    // it (staged): it must be checked and then finished with `merge.continue`.
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(work.rev("HEAD"), mine);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("resolved\n"));
    assert!(work.path().join(".git/MERGE_HEAD").exists());
    r.run("merge.continue", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD^1"), mine);
}

#[test]
fn rebase_never_deletes_an_ignored_file_added_and_removed_by_local_commits() {
    // C1 commits `x.env` by mistake, C2 removes it and ignores it; the real file stays in the
    // worktree. The rebase would reapply C1 (writes `x.env`) and C2 (deletes it).
    // The same for a renamed file (`y.env` -> `z.txt`) and for one removed and recreated.
    for case in ["added_then_removed", "renamed", "recreated"] {
        let Setup {
            other,
            work,
            _remote: _keep,
            ..
        } = setup();
        let file = match case {
            "added_then_removed" => "x.env",
            "renamed" => "y.env",
            _ => "w.env",
        };
        work.commit_file(file, "by mistake\n", "C1");
        match case {
            "renamed" => {
                work.git(&["mv", file, "z.txt"]);
                work.commit("C2: rename");
            }
            "recreated" => {
                work.git(&["rm", "--quiet", file]);
                work.commit("C2: remove");
                work.commit_file(file, "again\n", "C3: put back");
                work.git(&["rm", "--quiet", file]);
                work.commit("C4: remove again");
            }
            _ => {
                work.git(&["rm", "--quiet", file]);
                work.commit("C2: remove");
            }
        }
        work.write(".git/info/exclude", "*.env\n");
        work.write(file, "MYSECRET\n");
        publish(&other, "b.txt", "b\n", "from remote");
        let head = work.rev("HEAD");
        let (r, mut s) = open(&work);
        let err = r
            .run("remote.pull", &mut s, json!({ "strategy": "rebase" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{case}: {err}");
        assert!(err.to_string().contains(file), "{case}: {err}");
        assert_eq!(read(&work, file).as_deref(), Some("MYSECRET\n"), "{case}");
        assert_eq!(work.rev("HEAD"), head, "{case}");
        assert_eq!(backups(&work), Vec::<String>::new(), "{case}");
        // The same with `--rebase-merges`.
        work.git(&["config", "pull.rebase", "merges"]);
        let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{case}: {err}");
        assert_eq!(read(&work, file).as_deref(), Some("MYSECRET\n"), "{case}");
        // The merge does not go through those trees: the file stays and the pull succeeds.
        let out = pull(&r, &mut s, json!({ "strategy": "merge" }));
        assert_eq!(out["strategy"], "merge");
        assert_eq!(read(&work, file).as_deref(), Some("MYSECRET\n"), "{case}");
    }
}

#[test]
fn rebase_never_deletes_an_ignored_file_modified_locally_and_removed_upstream() {
    // The base tracks `x.env`; the upstream removes it (and ignores it). Locally one commit
    // modifies it and a later one removes it; the real file stays in the worktree, ignored. The
    // rebase would stop on a modify/delete conflict writing `x.env`, and the abort would
    // delete it.
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "x.env", "base\n", "base with x.env");
    work.git(&["fetch", "--quiet", "origin"]);
    work.git(&["merge", "--quiet", "--ff-only", "origin/main"]);
    other.git(&["rm", "--quiet", "x.env"]);
    other.commit("remove x.env");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    work.commit_file("x.env", "modified\n", "C1: modify");
    work.git(&["rm", "--quiet", "x.env"]);
    work.commit("C2: remove");
    work.write(".git/info/exclude", "*.env\n");
    work.write("x.env", "MYSECRET\n");
    let head = work.rev("HEAD");
    let (r, mut s) = open(&work);
    let err = r
        .run("remote.pull", &mut s, json!({ "strategy": "rebase" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("x.env"), "{err}");
    assert_eq!(read(&work, "x.env").as_deref(), Some("MYSECRET\n"));
    assert_eq!(work.rev("HEAD"), head);
    assert_eq!(backups(&work), Vec::<String>::new());
}
