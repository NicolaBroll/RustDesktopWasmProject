//! The interactive rebase editor on the `MemoryBackend`: editing the plan, the preview, the run
//! (same commits as the engine tests of S32 with the same plan), the confirmation for published
//! commits, refusals and a stop on a conflict.

use super::*;
use project_domain::{Change, ChangedFile, Commit, Signature};
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn sig(n: u64) -> Signature {
    Signature {
        name: "Ada".into(),
        email: "ada@example.com".into(),
        time: 1_700_000_000 + i64::try_from(n).unwrap() * 60,
        offset_min: 0,
    }
}

fn blob(repo: &mut MemoryBackend, content: &str) -> Oid {
    let hash = content.bytes().fold(1_469_598_103_934_665_603_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1_099_511_628_211)
    });
    let id = oid(hash);
    repo.set_blob(id, content);
    id
}

fn commit(
    repo: &mut MemoryBackend,
    n: u64,
    parents: &[u64],
    summary: &str,
    files: &[(&str, Option<&str>, &str)],
) {
    let changes = files
        .iter()
        .map(|(path, old, new)| {
            let old_oid = old.map(|c| blob(repo, c));
            ChangedFile {
                path: RepoPath::from(*path),
                change: if old.is_some() {
                    Change::Modified
                } else {
                    Change::Added
                },
                original: None,
                score: None,
                old_mode: if old.is_some() { 0o100644 } else { 0 },
                new_mode: 0o100644,
                old_oid: old_oid.unwrap_or(Oid::from_bytes([0; Oid::LEN])),
                new_oid: blob(repo, new),
            }
        })
        .collect();
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parents.iter().map(|p| oid(*p)).collect(),
        author: sig(n),
        committer: sig(n),
        summary: summary.into(),
        body: String::new(),
    });
    repo.set_changes(oid(n), changes);
}

/// The history of the S32 engine tests (`crates/engine/src/builtin/rebase_tests.rs`): `main` =
/// base, one, two, three; `merged` has a merge commit; with `four` also `c7` (adds `d.txt`).
fn repo(four: bool) -> MemoryBackend {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    commit(
        &mut repo,
        1,
        &[],
        "base",
        &[("a.txt", None, "base\n"), ("b.txt", None, "b\n")],
    );
    commit(
        &mut repo,
        2,
        &[1],
        "one",
        &[("a.txt", Some("base\n"), "one\n")],
    );
    commit(&mut repo, 3, &[2], "two", &[("c.txt", None, "c\n")]);
    commit(
        &mut repo,
        4,
        &[3],
        "three",
        &[("a.txt", Some("one\n"), "three\n")],
    );
    commit(&mut repo, 5, &[1], "side", &[("n.txt", None, "n\n")]);
    commit(
        &mut repo,
        6,
        &[4, 5],
        "Merge branch 'side'",
        &[("n.txt", None, "n\n")],
    );
    commit(&mut repo, 7, &[4], "four", &[("d.txt", None, "d\n")]);
    repo.set_branch("main", oid(if four { 7 } else { 4 }));
    repo.set_branch("merged", oid(6));
    repo.set_head_branch("main");
    repo
}

fn opened(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(opener));
    registry
        .run("repo.open", &mut session, json!({ "path": "r" }))
        .unwrap();
    (registry, session)
}

fn engine(r: &Registry, s: &mut Session, id: &str, params: Value) -> Result<Value, String> {
    r.run(id, s, params).map_err(|e| e.text())
}

/// Opens the editor on `base` and loads the plan like the executor.
fn open(r: &Registry, s: &mut Session, base: u64) -> RebaseBox {
    let mut b = RebaseBox::default();
    let params = b.open(oid(base), format!("c{}", base + 1)).unwrap();
    assert_eq!(params, json!({ "base": oid(base).to_string() }));
    b.receive_plan(engine(r, s, PLAN, params));
    b
}

/// Sends `rebase.start` with `params` and passes the response to the box.
fn start(r: &Registry, s: &mut Session, b: &mut RebaseBox, params: Value) -> Received {
    let result = r.run(START, s, params);
    let (kind, detail) = match &result {
        Err(e) => (Some(ErrorKind::of(e)), ErrorKind::detail(e)),
        Ok(_) => (None, None),
    };
    b.receive_start(result.map_err(|e| e.text()), kind, detail)
}

fn done(received: Received) -> Outcome {
    match received {
        Received::Done(outcome) => *outcome,
        other => panic!("expected Done, got {other:?}"),
    }
}

/// `(summary, body)` of the commits of `main` after `c1`, oldest first.
fn history(r: &Registry, s: &mut Session) -> Vec<(String, String)> {
    let log = engine(r, s, "repo.log", json!({ "rev": "main" })).unwrap();
    let mut out: Vec<(String, String)> = log["commits"]
        .as_array()
        .unwrap()
        .iter()
        .take_while(|c| c["id"] != oid(1).to_string())
        .map(|c| {
            (
                c["summary"].as_str().unwrap().to_owned(),
                c["body"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    out.reverse();
    out
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

fn order(b: &RebaseBox) -> Vec<Oid> {
    b.editor().unwrap().rows.iter().map(|r| r.commit).collect()
}

#[test]
fn editing_the_plan() {
    let (r, mut s) = opened(repo(true));
    let mut b = open(&r, &mut s, 1);
    let editor = b.editor().unwrap();
    assert_eq!(editor.phase, Phase::Editing);
    assert_eq!(editor.branch, "main");
    assert_eq!(order(&b), [oid(2), oid(3), oid(4), oid(7)]);
    assert_eq!(b.editor().unwrap().check(), Check::Noop);
    assert!(b.run().is_none(), "a plan that changes nothing is not sent");

    // Dragging: into the gap above row `to`; a drop on its own gaps changes nothing.
    b.move_step(3, 0);
    assert_eq!(order(&b), [oid(7), oid(2), oid(3), oid(4)]);
    assert_eq!(b.editor().unwrap().selected, Some(0));
    b.move_step(0, 4);
    assert_eq!(order(&b), [oid(2), oid(3), oid(4), oid(7)]);
    b.move_step(1, 1);
    b.move_step(1, 2);
    assert_eq!(order(&b), [oid(2), oid(3), oid(4), oid(7)]);
    b.move_step(0, 2);
    assert_eq!(order(&b), [oid(3), oid(2), oid(4), oid(7)]);
    assert_eq!(b.editor().unwrap().check(), Check::Ready);

    // Squash first: nothing above to join it into.
    b.set_action(0, RebaseAction::Squash);
    let Check::Invalid { step, text } = b.editor().unwrap().check() else {
        panic!("invalid expected");
    };
    assert_eq!(step, Some(0));
    assert!(text.contains("Step 1"), "{text}");
    assert!(b.editor().unwrap().preview().is_empty());
    assert!(b.run().is_none());

    // Reword starts from the summary; followed by a squash it is refused.
    b.set_action(0, RebaseAction::Reword);
    assert_eq!(b.editor().unwrap().rows[0].message, "two");
    assert_eq!(b.editor().unwrap().selected, Some(0));
    b.set_action(1, RebaseAction::Fixup);
    assert!(matches!(
        b.editor().unwrap().check(),
        Check::Invalid { step: Some(0), .. }
    ));
    b.set_action(1, RebaseAction::Pick);

    // A message only where allowed: the last squash/fixup of a group, through drops.
    b.set_action(2, RebaseAction::Squash);
    b.set_action(3, RebaseAction::Drop);
    let editor = b.editor().unwrap();
    assert!(editor.message_allowed(0));
    assert!(!editor.message_allowed(1));
    assert!(editor.message_allowed(2));
    assert!(!editor.message_allowed(3));
    b.set_message(1, "ignored on a pick".into());
    b.set_message(0, "  ".into());
    assert!(matches!(
        b.editor().unwrap().check(),
        Check::Invalid { step: Some(0), .. }
    ));
    b.set_message(0, "TWO".into());
    let plan = b.editor().unwrap().plan();
    assert_eq!(plan.steps[0].message.as_deref(), Some("TWO"));
    assert_eq!(plan.steps[1].message, None);
    assert_eq!(plan.steps[2].message, None);

    // The preview: reworded, then one + three combined; four dropped.
    let preview = b.editor().unwrap().preview();
    assert_eq!(preview.len(), 2);
    assert_eq!(preview[0].summary, "TWO");
    assert_eq!(preview[0].source, MessageSource::New);
    assert_eq!(preview[1].sources, [oid(2), oid(4)]);
    assert_eq!(preview[1].summary, "one + three");
    assert_eq!(preview[1].source, MessageSource::Combined);
    b.set_action(2, RebaseAction::Fixup);
    let preview = b.editor().unwrap().preview();
    assert_eq!(preview[1].summary, "one");
    assert_eq!(preview[1].source, MessageSource::Original);

    // Everything dropped.
    for i in 0..4 {
        b.set_action(i, RebaseAction::Drop);
    }
    assert!(matches!(
        b.editor().unwrap().check(),
        Check::Invalid { step: None, .. }
    ));
    b.close();
    assert!(b.editor().is_none());
}

/// Reorder + squash + drop + reword from the editor: the same commits as the same plan sent
/// straight to the engine.
#[test]
fn run_reorder_squash_drop_reword() {
    let (r, mut s) = opened(repo(true));
    let mut b = open(&r, &mut s, 1);
    b.move_step(1, 0); // two, one, three, four
    b.set_action(0, RebaseAction::Reword);
    b.set_message(0, "TWO".into());
    b.set_action(2, RebaseAction::Squash);
    b.set_action(3, RebaseAction::Drop);
    let (params, label) = b.run().expect("valid plan");
    assert_eq!(label, "Interactive rebase of “main”");
    assert_eq!(b.editor().unwrap().phase, Phase::Sending);
    assert!(b.run().is_none(), "one run at a time");
    b.close();
    assert!(b.editor().is_some(), "not closed while sending");
    let id = |n| oid(n).to_string();
    assert_eq!(
        params,
        json!({ "base": id(1), "steps": [
            { "action": "reword", "commit": id(3), "message": "TWO" },
            { "action": "pick", "commit": id(2) },
            { "action": "squash", "commit": id(4) },
            { "action": "drop", "commit": id(7) },
        ]})
    );
    let outcome = done(start(&r, &mut s, &mut b, params.clone()));
    assert!(b.editor().is_none(), "the editor closes when done");
    assert!(outcome.moved && !outcome.show_changes);
    assert!(outcome.history.can_undo);
    assert!(
        outcome
            .notice
            .contains("4 commits became 2, the branch is at"),
        "{}",
        outcome.notice
    );
    assert!(
        outcome.notice.contains("refs/project/backup/rebase-1"),
        "{}",
        outcome.notice
    );
    assert_eq!(outcome.warning, None);
    let expected = pairs(&[("TWO", ""), ("one", "three")]);
    assert_eq!(history(&r, &mut s), expected);

    // The same plan straight to the engine, on a fresh repo.
    let (r2, mut s2) = opened(repo(true));
    engine(&r2, &mut s2, START, params).unwrap();
    assert_eq!(history(&r2, &mut s2), expected);
}

/// The plan of the S32 test `reorder_and_fixup_with_message_then_undo_redo`, built by dragging.
#[test]
fn run_gives_the_same_commits_as_the_engine_test() {
    let (r, mut s) = opened(repo(false));
    let mut b = open(&r, &mut s, 1);
    b.move_step(0, 2); // two, one, three
    b.set_action(2, RebaseAction::Fixup);
    b.set_message(2, "one and three\n\nwith a body".into());
    let (params, _) = b.run().unwrap();
    done(start(&r, &mut s, &mut b, params));
    assert_eq!(
        history(&r, &mut s),
        pairs(&[("two", ""), ("one and three", "with a body")])
    );
}

#[test]
fn published_commits_ask_for_confirmation() {
    let mut published = repo(false);
    published.set_remote_branch("origin/main", oid(2));
    let (r, mut s) = opened(published);
    let mut b = open(&r, &mut s, 1);
    let rows = &b.editor().unwrap().rows;
    assert!(rows[0].published && !rows[1].published && !rows[2].published);
    b.move_step(1, 0);
    let (params, _) = b.run().unwrap();
    assert!(params.get("confirm").is_none());
    assert!(matches!(
        start(&r, &mut s, &mut b, params),
        Received::Confirm
    ));
    let Phase::Confirm { reason } = &b.editor().unwrap().phase else {
        panic!("confirmation expected");
    };
    assert!(reason.contains(&oid(2).short()), "{reason}");
    assert_eq!(history(&r, &mut s).len(), 3, "nothing written");

    // Back to the plan, then run again and confirm.
    b.cancel_confirm();
    assert_eq!(b.editor().unwrap().phase, Phase::Editing);
    let (params, _) = b.run().unwrap();
    assert!(matches!(
        start(&r, &mut s, &mut b, params),
        Received::Confirm
    ));
    assert!(b.run().is_none(), "not while confirming");
    let (params, _) = b.confirm().unwrap();
    assert_eq!(params["confirm"], true);
    done(start(&r, &mut s, &mut b, params));
    assert_eq!(
        history(&r, &mut s),
        pairs(&[("two", ""), ("one", ""), ("three", "")])
    );
}

#[test]
fn refusals_are_shown_in_the_editor() {
    // Merges in the range: the plan cannot be read.
    let mut merged = repo(false);
    merged.set_head_branch("merged");
    let (r, mut s) = opened(merged);
    let b = open(&r, &mut s, 1);
    let editor = b.editor().unwrap();
    assert_eq!(editor.phase, Phase::Unavailable);
    assert!(
        editor.error.as_deref().unwrap().contains("merge commits"),
        "{:?}",
        editor.error
    );

    // Local changes: refused at the run, the editor stays with the reason.
    let mut dirty = repo(false);
    dirty.set_worktree_file("a.txt", "changed\n");
    let a = blob(&mut dirty, "three\n");
    dirty.set_entries(vec![project_domain::StatusEntry {
        path: RepoPath::from("a.txt"),
        index: Change::Unmodified,
        worktree: Change::Modified,
        kind: project_domain::EntryKind::Ordinary(project_domain::TrackedInfo {
            submodule: None,
            mode_head: 0o100644,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: a,
            oid_index: a,
        }),
    }]);
    let (r, mut s) = opened(dirty);
    let mut b = open(&r, &mut s, 1);
    b.set_action(1, RebaseAction::Drop);
    let (params, _) = b.run().unwrap();
    assert!(matches!(
        start(&r, &mut s, &mut b, params),
        Received::Inline
    ));
    let editor = b.editor().unwrap();
    assert_eq!(editor.phase, Phase::Editing);
    assert!(
        editor
            .error
            .as_deref()
            .unwrap()
            .starts_with("Not possible: "),
        "{:?}",
        editor.error
    );
    // Editing again clears the reason.
    b.set_action(1, RebaseAction::Pick);
    assert_eq!(b.editor().unwrap().error, None);
}

#[test]
fn a_conflict_stops_and_opens_the_changes() {
    let (r, mut s) = opened(repo(false));
    let mut b = open(&r, &mut s, 1);
    b.move_step(2, 0); // three before one: a conflict on a.txt
    let (params, _) = b.run().unwrap();
    let outcome = done(start(&r, &mut s, &mut b, params));
    assert!(outcome.show_changes && outcome.moved);
    assert!(
        outcome
            .notice
            .starts_with("Rebase of “main” stopped: 1 conflicted file"),
        "{}",
        outcome.notice
    );
    let status = outcome.status.unwrap();
    assert!(status.operation.is_some() && status.has_conflicts());
}
