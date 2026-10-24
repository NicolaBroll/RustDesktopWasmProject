//! Integration tests of `rebase.plan`, `rebase.start`, `rebase.continue`, `rebase.abort` (S32)
//! with the `CliBackend`: every plan action (reorder, reword, squash with and without message,
//! fixup, drop) with undo and redo, a conflict that stays in progress and ends with
//! `rebase.continue` (also resolved to nothing) or `rebase.abort`, a `commit-msg` hook that
//! rejects the message of a pause, failing signature, refusals (local changes, merges in the
//! range, published commits without confirmation, untracked files in the way, detached HEAD),
//! user configuration that would change the rebase, a rebase started from the terminal.

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

fn in_progress(repo: &Repo) -> bool {
    repo.path().join(".git/rebase-merge").exists()
}

/// The messages (`%B`, trimmed) of `base..main`, oldest first.
fn messages(repo: &Repo, base: Oid) -> Vec<String> {
    let range = format!("{base}..main");
    let out = repo.git(&["log", "--reverse", "--format=%B%x00", &range]);
    out.split('\0')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
        .collect()
}

fn step(action: &str, commit: Oid) -> Value {
    json!({ "action": action, "commit": commit.to_string() })
}

fn with_message(mut step: Value, message: &str) -> Value {
    step["message"] = json!(message);
    step
}

fn plan(base: Oid, steps: Vec<Value>) -> Value {
    json!({ "base": base.to_string(), "steps": steps })
}

/// `c1` (`a.txt` = "base", `b.txt`), `c2` "one" (`a.txt` = "one"), `c3` "two" by Cleo (adds
/// `c.txt`), `c4` "three" (`a.txt` = "three"); `main` at `c4`.
struct Linear {
    repo: Repo,
    c1: Oid,
    c2: Oid,
    c3: Oid,
    c4: Oid,
}

fn linear() -> Linear {
    let repo = Repo::new();
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    let c1 = repo.commit_file("a.txt", "base\n", "base");
    let c2 = repo.commit_file("a.txt", "one\n", "one");
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    repo.git(&[
        "commit",
        "--quiet",
        "--author=Cleo <cleo@example.com>",
        "-m",
        "two",
    ]);
    let c3 = repo.rev("HEAD");
    let c4 = repo.commit_file("a.txt", "three\n", "three");
    Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    }
}

#[test]
fn plan_lists_the_range() {
    let Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    } = linear();
    repo.git(&["update-ref", "refs/remotes/origin/main", &c2.to_string()]);
    // A tag whose name starts with `-` as `base`.
    repo.git(&["update-ref", "refs/tags/-x", &c1.to_string()]);
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "rebase.plan", json!({ "base": "-x" }));
    assert_eq!(out["base"], c1.to_string());
    assert_eq!(out["branch"], "main");
    let ids: Vec<&str> = out["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["commit"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [c2.to_string(), c3.to_string(), c4.to_string()]);
    assert_eq!(out["steps"][2]["summary"], "three");
    assert_eq!(out["published"], json!([c2.to_string()]));
}

#[test]
fn reorder_and_fixup_with_message_then_undo_and_redo() {
    let Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    } = linear();
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            c1,
            vec![
                step("pick", c3),
                step("pick", c2),
                with_message(step("fixup", c4), "one and three\n\nwith a body"),
            ],
        ),
    );
    assert_eq!(out["in_progress"], false, "{out}");
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(out["before"], c4.to_string());
    let after = repo.rev("main");
    assert_eq!(out["after"], after.to_string());
    assert_eq!(messages(&repo, c1), ["two", "one and three\n\nwith a body"]);
    // The author of the moved commit stays.
    assert_eq!(repo.git(&["log", "-1", "--format=%an", "main~1"]), "Cleo");
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("three\n"));
    assert_eq!(read(&repo, "c.txt").as_deref(), Some("c\n"));
    assert!(clean(&repo));
    assert!(!in_progress(&repo));
    assert!(!repo.path().join(".git/project-rebase-todo").exists());
    assert_eq!(repo.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    let backup = out["backup"].as_str().unwrap();
    assert!(
        backup.starts_with("refs/project/backup/rebase-"),
        "{backup}"
    );
    assert_eq!(repo.rev(backup), c4);
    assert_eq!(out["can_undo"], true);
    let undone = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c4);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("three\n"));
    assert!(clean(&repo));
    assert_eq!(undone["can_redo"], true);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), after);
}

#[test]
fn squash_without_message_joins_the_messages() {
    let Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    } = linear();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            c1,
            vec![step("pick", c2), step("squash", c3), step("pick", c4)],
        ),
    );
    assert_eq!(messages(&repo, c1), ["one\n\ntwo", "three"]);
    // The combined commit has the first commit's author.
    assert_eq!(repo.git(&["log", "-1", "--format=%an", "main~1"]), "Ada");
}

#[test]
fn squash_with_message_and_fixup_without() {
    let Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    } = linear();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            c1,
            vec![
                step("pick", c2),
                with_message(step("squash", c3), "one and two"),
                step("pick", c4),
            ],
        ),
    );
    assert_eq!(messages(&repo, c1), ["one and two", "three"]);
    run(&r, &mut s, "edit.undo", json!({}));
    run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            c1,
            vec![step("pick", c2), step("fixup", c3), step("fixup", c4)],
        ),
    );
    assert_eq!(messages(&repo, c1), ["one"]);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("three\n"));
    assert_eq!(read(&repo, "c.txt").as_deref(), Some("c\n"));
}

#[test]
fn reword_and_drop() {
    let Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    } = linear();
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            c1,
            vec![
                with_message(step("reword", c2), "ONE"),
                step("drop", c3),
                with_message(step("reword", c4), "THREE"),
            ],
        ),
    );
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(messages(&repo, c1), ["ONE", "THREE"]);
    assert_eq!(read(&repo, "c.txt"), None);
    assert!(clean(&repo));
    // The dropped commit stays reachable from the backup.
    let backup = out["backup"].as_str().unwrap();
    assert!(
        repo.git(&["merge-base", "--is-ancestor", &c3.to_string(), backup])
            .is_empty()
    );
}

/// `c4` before `c2`: `c4` changes `a.txt` from "one", which is not there yet; with a reword of
/// `c2` after it.
fn conflicted(linear: &Linear) -> (Registry, Session, Value) {
    let (r, mut s) = open(&linear.repo);
    let out = run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            linear.c1,
            vec![
                step("pick", linear.c4),
                with_message(step("reword", linear.c2), "ONE"),
                step("pick", linear.c3),
            ],
        ),
    );
    (r, s, out)
}

#[test]
fn conflict_stays_in_progress_then_continue() {
    let l = linear();
    let repo = &l.repo;
    let (r, mut s, out) = conflicted(&l);
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert!(
        out["message"].as_str().unwrap().contains("CONFLICT"),
        "{out}"
    );
    assert_eq!(out["after"], l.c4.to_string());
    assert_eq!(out["can_undo"], false);
    assert_eq!(repo.rev("main"), l.c4, "the branch moves only at the end");
    assert!(
        repo.path()
            .join(".git/rebase-merge/project-rebase.json")
            .exists()
    );
    assert!(!repo.path().join(".git/project-rebase-todo").exists());
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["continue"], "rebase.continue");
    assert_eq!(op["onto"], l.c1.to_string());
    assert!(r.run("edit.undo", &mut s, json!({})).is_err());
    let err = r.run("rebase.continue", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("a.txt"), "{err}");

    repo.write("a.txt", "three\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    // `c2` (base → one) on "three": a conflict again.
    let next = run(&r, &mut s, "rebase.continue", json!({}));
    assert_eq!(next["in_progress"], true, "{next}");
    assert_eq!(next["conflicts"], json!(["a.txt"]));
    assert_eq!(next["command"], "rebase.start");
    repo.write("a.txt", "three and one\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    // Then the pause of the reword with its message, and `c3`.
    let done = run(&r, &mut s, "rebase.continue", json!({}));
    assert_eq!(done["in_progress"], false, "{done}");
    assert_eq!(done["warnings"], json!([]));
    assert_eq!(done["before"], l.c4.to_string());
    assert_eq!(messages(repo, l.c1), ["three", "ONE", "two"]);
    assert_eq!(read(repo, "a.txt").as_deref(), Some("three and one\n"));
    assert!(clean(repo));
    assert!(!in_progress(repo));
    let history = run(&r, &mut s, "edit.history", json!({}));
    let last = history["done"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["command"], "rebase.continue");
    assert!(
        last["description"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Rebase of `main` (3 commits"),
        "{last}"
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), l.c4);
    assert_eq!(read(repo, "a.txt").as_deref(), Some("three\n"));
}

#[test]
fn a_step_resolved_to_nothing_is_dropped_by_git() {
    let l = linear();
    let repo = &l.repo;
    let (r, mut s, _) = conflicted(&l);
    // `a.txt` as HEAD (`c1`): nothing left of `c4`.
    repo.write("a.txt", "base\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let done = run(&r, &mut s, "rebase.continue", json!({}));
    assert_eq!(done["in_progress"], false, "{done}");
    let warnings = done["warnings"].to_string();
    assert!(warnings.contains("was dropped"), "{warnings}");
    assert!(
        warnings.contains("2 commits instead of the 3"),
        "{warnings}"
    );
    assert_eq!(messages(repo, l.c1), ["ONE", "two"]);
    // Still in the journal: undo brings back the branch.
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), l.c4);
}

#[test]
fn conflict_then_abort() {
    let l = linear();
    let repo = &l.repo;
    let (r, mut s, _) = conflicted(&l);
    repo.write("a.txt", "half resolved\n");
    let err = r.run("rebase.abort", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(err.to_string().contains("a.txt"), "{err}");
    assert!(in_progress(repo), "nothing changed");
    // An untracked file where the branch as it was has `c.txt`: git would refuse halfway, the
    // abort refuses first.
    repo.write("c.txt", "mine\n");
    let err = r
        .run("rebase.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("c.txt"), "{err}");
    assert!(in_progress(repo));
    assert!(backups(repo, "rebase-abort").is_empty());
    // Ignored, git would overwrite it silently: it goes in the backup.
    repo.write(".git/info/exclude", "c.txt\n");
    let aborted = run(&r, &mut s, "rebase.abort", json!({ "confirm": true }));
    let backup = aborted["backup"].as_str().unwrap();
    assert!(
        backup.starts_with("refs/project/backup/rebase-abort-"),
        "{aborted}"
    );
    assert!(
        aborted["warnings"].to_string().contains("c.txt"),
        "{aborted}"
    );
    assert_eq!(repo.rev("HEAD"), l.c4);
    assert_eq!(repo.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert!(!in_progress(repo));
    assert!(clean(repo), "{}", repo.git(&["status", "--porcelain"]));
    // The backup has the half resolution and the untracked file.
    assert_eq!(
        repo.git(&["show", &format!("{backup}:a.txt")]),
        "half resolved"
    );
    assert_eq!(repo.git(&["show", &format!("{backup}:c.txt")]), "mine");
    assert_eq!(aborted["can_undo"], false);
    for id in ["rebase.continue", "rebase.abort"] {
        let params = if id == "rebase.abort" {
            json!({ "confirm": true })
        } else {
            json!({})
        };
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(err.to_string().contains("not in progress"), "{id}: {err}");
    }
}

/// Installs the hook `name` with the script `body` (after `#!/bin/sh`).
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
fn a_rejected_message_leaves_the_rebase_at_the_pause() {
    let Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    } = linear();
    install_hook(
        &repo,
        "commit-msg",
        "if grep -q bad \"$1\"; then echo 'commit-msg: no bad words' >&2; exit 1; fi\n",
    );
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            c1,
            vec![
                with_message(step("reword", c2), "bad one"),
                step("pick", c3),
                step("pick", c4),
            ],
        ),
    );
    assert_eq!(out["in_progress"], true, "{out}");
    assert_eq!(out["conflicts"], json!([]));
    assert!(
        out["message"].as_str().unwrap().contains("no bad words"),
        "{out}"
    );
    assert_eq!(repo.rev("main"), c4);
    // Still rejected: still at the pause.
    let again = run(&r, &mut s, "rebase.continue", json!({}));
    assert_eq!(again["in_progress"], true);
    assert!(in_progress(&repo));
    std::fs::remove_file(repo.path().join(".git/hooks/commit-msg")).unwrap();
    let done = run(&r, &mut s, "rebase.continue", json!({}));
    assert_eq!(done["in_progress"], false, "{done}");
    assert_eq!(messages(&repo, c1), ["bad one", "two", "three"]);
    assert_eq!(done["can_undo"], true);
}

#[test]
fn a_failing_signature_stops_and_abort_puts_everything_back() {
    let Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    } = linear();
    repo.git(&["config", "commit.gpgsign", "true"]);
    repo.git(&["config", "gpg.program", "false"]);
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            c1,
            vec![step("pick", c3), step("pick", c2), step("pick", c4)],
        ),
    );
    assert_eq!(out["in_progress"], true, "{out}");
    assert!(!out["message"].as_str().unwrap().is_empty());
    assert_eq!(repo.rev("main"), c4);
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert_eq!(repo.rev(&backup), c4);
    run(&r, &mut s, "rebase.abort", json!({ "confirm": true }));
    assert_eq!(repo.rev("HEAD"), c4);
    assert!(clean(&repo));
    assert!(!in_progress(&repo));
    // The start backup stays (the rebase did not conclude).
    assert!(backups(&repo, "rebase").contains(&backup));
}

#[test]
fn refusals_change_nothing() {
    let Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    } = linear();
    let (r, mut s) = open(&repo);
    let reorder = plan(
        c1,
        vec![step("pick", c3), step("pick", c2), step("pick", c4)],
    );

    // Local changes: never autostash.
    repo.write("a.txt", "dirty\n");
    let err = r.run("rebase.start", &mut s, reorder.clone()).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("a.txt"), "{err}");
    repo.git(&["checkout", "--", "a.txt"]);

    // Invalid plan, plan that changes nothing.
    let err = r
        .run("rebase.start", &mut s, plan(c1, vec![step("squash", c2)]))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    let same = plan(
        c1,
        vec![step("pick", c2), step("pick", c3), step("pick", c4)],
    );
    let err = r.run("rebase.start", &mut s, same).unwrap_err();
    assert!(err.to_string().contains("change nothing"), "{err}");

    // Published commits: confirmation.
    repo.git(&["update-ref", "refs/remotes/origin/main", &c3.to_string()]);
    let err = r.run("rebase.start", &mut s, reorder.clone()).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(err.to_string().contains(&c2.to_string()[..7]), "{err}");
    assert_eq!(repo.rev("main"), c4);
    assert!(backups(&repo, "rebase").is_empty());
    let mut confirmed = reorder;
    confirmed["confirm"] = json!(true);
    let out = run(&r, &mut s, "rebase.start", confirmed);
    assert_eq!(out["in_progress"], false);
    run(&r, &mut s, "edit.undo", json!({}));

    // A merge in the range.
    repo.git(&["checkout", "--quiet", "-b", "side", &c1.to_string()]);
    repo.commit_file("n.txt", "n\n", "side");
    repo.git(&["checkout", "--quiet", "main"]);
    repo.git(&["merge", "--quiet", "--no-ff", "-m", "merge side", "side"]);
    let err = r
        .run("rebase.plan", &mut s, json!({ "base": c1.to_string() }))
        .unwrap_err();
    assert!(err.to_string().contains("merge commits"), "{err}");

    // Detached HEAD.
    repo.git(&["checkout", "--quiet", "--detach", &c4.to_string()]);
    let err = r
        .run("rebase.plan", &mut s, json!({ "base": c1.to_string() }))
        .unwrap_err();
    assert!(err.to_string().contains("detached"), "{err}");
}

#[test]
fn an_untracked_file_in_the_way_is_refused() {
    let repo = Repo::new();
    let base = repo.commit_file("a.txt", "a\n", "base");
    let add = repo.commit_file("gen.txt", "generated\n", "adds gen");
    repo.git(&["rm", "--quiet", "gen.txt"]);
    let rm = repo.commit("removes gen");
    let last = repo.commit_file("a.txt", "b\n", "last");
    // An ignored file where the range writes `gen.txt` (and then deletes it).
    repo.write(".git/info/exclude", "gen.txt\n");
    repo.write("gen.txt", "precious\n");
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "rebase.start",
            &mut s,
            plan(
                base,
                vec![
                    step("pick", add),
                    step("pick", rm),
                    with_message(step("reword", last), "LAST"),
                ],
            ),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("gen.txt"), "{err}");
    assert_eq!(read(&repo, "gen.txt").as_deref(), Some("precious\n"));
    assert_eq!(repo.rev("main"), last);
    assert!(backups(&repo, "rebase").is_empty());
}

#[test]
fn user_configuration_does_not_change_the_plan() {
    let Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    } = linear();
    for (key, value) in [
        ("rebase.autoSquash", "true"),
        ("rebase.updateRefs", "true"),
        ("rebase.abbreviateCommands", "true"),
        ("rebase.missingCommitsCheck", "ignore"),
        ("rebase.instructionFormat", "%an %s"),
        ("rebase.autoStash", "true"),
        ("sequence.editor", "false"),
        ("core.editor", "false"),
        ("commit.cleanup", "verbatim"),
    ] {
        repo.git(&["config", key, value]);
    }
    // A branch inside the range: `rebase.updateRefs` would move it outside the journal.
    repo.git(&["branch", "keep", &c2.to_string()]);
    // A commit message that `autoSquash` would read as an instruction.
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            c1,
            vec![
                with_message(step("reword", c3), "fixup! one"),
                step("pick", c2),
                step("squash", c4),
            ],
        ),
    );
    // `commit.cleanup=verbatim` would keep git's squash template ("# This is a combination...").
    assert_eq!(messages(&repo, c1), ["fixup! one", "one\n\nthree"]);
    assert_eq!(repo.rev("keep"), c2);
}

#[test]
fn messages_with_hash_lines_survive_commit_cleanup_strip() {
    let Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    } = linear();
    repo.git(&["config", "commit.cleanup", "strip"]);
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            c1,
            vec![
                with_message(step("reword", c2), "#1 title\n\n# body"),
                step("pick", c3),
                with_message(step("squash", c4), "# squashed"),
            ],
        ),
    );
    assert_eq!(messages(&repo, c1), ["#1 title\n\n# body", "# squashed"]);
}

/// `c1`, `c2` "one" (`a.txt`), `c3` adds `d/f`, `c4` "three" (`a.txt`); the plan puts `c4` first:
/// a conflict with HEAD on `c1`, where there is no `d/`.
fn folder_scenario() -> (Repo, Oid, Registry, Session) {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "base\n", "base");
    let c2 = repo.commit_file("a.txt", "one\n", "one");
    let c3 = repo.commit_file("d/f", "f\n", "adds d/f");
    let c4 = repo.commit_file("a.txt", "three\n", "three");
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            c1,
            vec![step("pick", c4), step("pick", c2), step("pick", c3)],
        ),
    );
    assert_eq!(out["in_progress"], true);
    (repo, c4, r, s)
}

#[test]
fn abort_with_a_file_in_place_of_a_folder() {
    // Untracked `d` where the branch as it was has `d/f`: refused before writing.
    let (repo, c4, r, mut s) = folder_scenario();
    repo.write("d", "untracked\n");
    let err = r
        .run("rebase.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`d`"), "{err}");
    assert!(in_progress(&repo));
    assert!(backups(&repo, "rebase-abort").is_empty());
    // Ignored: git would replace it silently, it goes in the backup with a warning.
    repo.write(".git/info/exclude", "d\n");
    let aborted = run(&r, &mut s, "rebase.abort", json!({ "confirm": true }));
    assert!(aborted["warnings"].to_string().contains("`d`"), "{aborted}");
    let backup = aborted["backup"].as_str().unwrap();
    assert_eq!(repo.git(&["show", &format!("{backup}:d")]), "untracked");
    assert_eq!(repo.rev("HEAD"), c4);
    assert_eq!(read(&repo, "d/f").as_deref(), Some("f\n"));
}

#[test]
fn a_message_left_behind_by_a_terminal_continue_is_reported() {
    let Linear {
        repo,
        c1,
        c2,
        c3,
        c4,
    } = linear();
    install_hook(
        &repo,
        "commit-msg",
        "if grep -q bad \"$1\"; then echo 'no bad words' >&2; exit 1; fi\n",
    );
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "rebase.start",
        plan(
            c1,
            vec![
                with_message(step("reword", c3), "bad two"),
                step("pick", c4),
                step("pick", c2),
            ],
        ),
    );
    assert_eq!(out["in_progress"], true);
    // From the terminal: the pause is left without its message, then `c4` conflicts.
    repo.git_fails(&["rebase", "--continue"]);
    let mut last = Value::Null;
    for content in ["three\n", "three and one\n"] {
        repo.write("a.txt", content);
        run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
        last = run(&r, &mut s, "rebase.continue", json!({}));
    }
    assert_eq!(last["in_progress"], false, "{last}");
    let warnings = last["warnings"].to_string();
    assert!(warnings.contains("were not applied"), "{warnings}");
    assert_eq!(last["can_undo"], true);
    assert!(!in_progress(&repo));
    assert_eq!(messages(&repo, c1)[0], "two");
}

#[test]
fn a_pause_moved_from_the_terminal_does_not_get_the_message() {
    let l = linear();
    let repo = &l.repo;
    let (r, mut s, _) = conflicted(&l);
    // The remaining todo is `pick c2`, `break`, `pick c3`: the pause moved after `c3`, and a
    // second one after `c2`.
    let todo = format!("pick {}\nbreak\npick {}\nbreak\n", l.c3, l.c2);
    std::fs::write(repo.path().join(".git/rebase-merge/git-rebase-todo"), todo).unwrap();
    repo.write("a.txt", "three\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let next = run(&r, &mut s, "rebase.continue", json!({}));
    assert!(
        next["warnings"].to_string().contains("did not follow"),
        "{next}"
    );
    assert_eq!(next["in_progress"], true, "`c2` conflicts");
    repo.write("a.txt", "three and one\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let done = run(&r, &mut s, "rebase.continue", json!({}));
    assert_eq!(done["in_progress"], false, "{done}");
    assert_eq!(messages(repo, l.c1), ["three", "two", "one"]);
}

#[test]
fn a_record_that_does_not_match_is_ignored() {
    let l = linear();
    let repo = &l.repo;
    let (r, mut s, _) = conflicted(&l);
    // The record says the branch was elsewhere: it is not this rebase's.
    let file = repo.path().join(".git/rebase-merge/project-rebase.json");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.replace(&l.c4.to_string(), &l.c3.to_string())).unwrap();
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["continue"], "rebase.continue");
    let mut last = Value::Null;
    for content in ["three\n", "three and one\n", ""] {
        if !content.is_empty() {
            repo.write("a.txt", content);
            run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
        }
        last = run(&r, &mut s, "rebase.continue", json!({}));
        assert_eq!(last["command"], json!(null), "{last}");
        if last["in_progress"] == false {
            break;
        }
    }
    assert_eq!(last["in_progress"], false, "{last}");
    assert!(
        last["warnings"].to_string().contains("does not match"),
        "{last}"
    );
    assert_eq!(last["can_undo"], false);
    // Without the record the reword's message was not applied.
    assert_eq!(messages(repo, l.c1), ["three", "one", "two"]);
}

#[test]
fn a_rebase_from_the_terminal_concludes_without_journal() {
    let Linear { repo, c1, .. } = linear();
    repo.git(&["checkout", "--quiet", "-b", "other", &c1.to_string()]);
    repo.commit_file("a.txt", "other\n", "other");
    repo.git(&["checkout", "--quiet", "main"]);
    repo.git_fails(&["rebase", "other"]);
    let (r, mut s) = open(&repo);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["continue"], "rebase.continue");
    repo.write("a.txt", "merged\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    // `c2` resolved; `c3` applies; `c4` conflicts again.
    let next = run(&r, &mut s, "rebase.continue", json!({}));
    assert_eq!(next["command"], json!(null));
    assert_eq!(next["branch"], "main");
    assert_eq!(next["in_progress"], true, "{next}");
    repo.write("a.txt", "merged three\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let done = run(&r, &mut s, "rebase.continue", json!({}));
    assert_eq!(done["in_progress"], false, "{done}");
    assert!(
        done["warnings"]
            .to_string()
            .contains("not started by the app"),
        "{done}"
    );
    assert_eq!(done["can_undo"], false);
    assert!(!in_progress(&repo));
    assert_eq!(repo.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
}
