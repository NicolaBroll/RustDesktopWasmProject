//! Cherry-pick and revert from the UI on the `MemoryBackend`: start, panel with the commands of
//! `repo.operation`, "Continue" and two-step abort.

use super::super::{Dialog, Kind, OPERATION};
use super::*;
use crate::discard::Phase;
use project_domain::{Change, ChangedFile, Commit, Operation, RepoPath, Signature, Status};
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

/// Commit `n` ("c<n>") with parent `parent` that brings `a.txt` to the blob with content `text`.
fn commit(repo: &mut MemoryBackend, n: u64, parent: Option<u64>, text: &str) {
    let old = parent.map(|p| oid(100 + p));
    repo.set_blob(oid(100 + n), text.to_owned());
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parent.map(oid).into_iter().collect(),
        author: sig(n),
        committer: sig(n),
        summary: format!("c{n}"),
        body: String::new(),
    });
    repo.set_changes(
        oid(n),
        vec![ChangedFile {
            path: RepoPath::from("a.txt"),
            change: if old.is_some() {
                Change::Modified
            } else {
                Change::Added
            },
            original: None,
            score: None,
            old_mode: if old.is_some() { 0o100644 } else { 0 },
            new_mode: 0o100644,
            old_oid: old.unwrap_or(Oid::from_bytes([0; Oid::LEN])),
            new_oid: oid(100 + n),
        }],
    );
}

/// `main` on `c2` and `other` on `c3`, both from `c1` and different on `a.txt`: the cherry-pick
/// of `c3` on `main` stops on a conflict; the revert of `c2` succeeds.
fn opened() -> (Registry, Session) {
    opened_on(2)
}

/// Like [`opened`], with `main` on `c<tip>`; `c4` (on `c2`) changes `a.txt` again, so on it the
/// revert of `c2` stops on a conflict.
fn opened_on(tip: u64) -> (Registry, Session) {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    commit(&mut repo, 1, None, "base\n");
    commit(&mut repo, 2, Some(1), "ours\n");
    commit(&mut repo, 3, Some(1), "theirs\n");
    commit(&mut repo, 4, Some(2), "later\n");
    repo.set_branch("main", oid(tip));
    repo.set_branch("other", oid(3));
    repo.set_head_branch("main");
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(opener));
    registry
        .run("repo.open", &mut session, json!({ "path": "r" }))
        .unwrap();
    (registry, session)
}

/// Runs `op` like the executor and passes the response to `receive`.
fn run(r: &Registry, s: &mut Session, b: &mut MergeBox, op: Op, params: Value) -> Received {
    let result = r.run(op.command(), s, params);
    let (kind, detail) = match &result {
        Err(e) => (Some(ErrorKind::of(e)), ErrorKind::detail(e)),
        Ok(_) => (None, None),
    };
    b.receive(op, result.map_err(|e| e.text()), kind, detail)
}

/// Rereads status and operation like the UI.
fn refresh(r: &Registry, s: &mut Session, b: &mut MergeBox, seq: u64) {
    let status: Status =
        serde_json::from_value(r.run("repo.status", s, json!({})).unwrap()).unwrap();
    if b.want_info(status.operation, seq) {
        b.receive_info(r.run(OPERATION, s, json!({})).map_err(|e| e.text()));
    }
}

fn done(received: Received) -> Outcome {
    match received {
        Received::Done(outcome) => *outcome,
        other => panic!("expected Done, got {other:?}"),
    }
}

/// Starts `pick` of a commit with a single parent.
fn start(b: &mut MergeBox, pick: Pick, n: u64) -> Value {
    let (params, label) = b
        .start_pick(
            pick,
            oid(n),
            format!("c{n}"),
            "main".into(),
            vec![(oid(1), oid(1).short(), None)],
        )
        .expect("starts at once");
    assert!(label.ends_with("onto “main”"), "{label}");
    assert_eq!(params, json!({ "rev": oid(n).to_string() }));
    params
}

#[test]
fn conflicted_cherry_pick_is_resolved_and_finished_from_the_panel() {
    let (r, mut s) = opened();
    let mut b = MergeBox::default();
    refresh(&r, &mut s, &mut b, 1);

    let params = start(&mut b, Pick::CherryPick, 3);
    assert!(b.busy());
    let outcome = done(run(&r, &mut s, &mut b, Op::Pick(Pick::CherryPick), params));
    assert!(!outcome.moved);
    assert!(outcome.show_changes);
    assert!(
        outcome.notice.contains("1 conflicted file")
            && outcome.notice.contains("“Continue cherry-pick…”"),
        "{}",
        outcome.notice
    );

    // The panel takes the commands from `repo.operation`.
    refresh(&r, &mut s, &mut b, 2);
    let info = b.info().expect("cherry-pick in progress");
    assert_eq!(info.operation, Some(Operation::CherryPick));
    assert!(
        info.title().starts_with("Cherry-pick in progress"),
        "{}",
        info.title()
    );
    assert_eq!(
        info.continue_command.as_deref(),
        Some("cherry_pick.continue")
    );
    assert_eq!(info.abort_command.as_deref(), Some("cherry_pick.abort"));

    // With the conflict the engine refuses: the error stays in the dialog.
    assert!(b.open_continue());
    let (op, params, _) = b.submit_continue().unwrap();
    assert_eq!(op, Op::Continue(Kind::CherryPick));
    assert!(matches!(
        run(&r, &mut s, &mut b, op, params),
        Received::Inline
    ));
    b.close();

    // Resolved and marked: it concludes.
    r.run("index.stage", &mut s, json!({ "paths": ["a.txt"] }))
        .unwrap();
    refresh(&r, &mut s, &mut b, 3);
    assert!(b.info().unwrap().ready);
    assert!(b.open_continue());
    let Some(Dialog::Continue(dialog)) = b.dialog() else {
        panic!("Continue dialog");
    };
    assert_eq!(dialog.kind, Kind::CherryPick);
    assert!(!dialog.is_blank(), "message prepared by git");
    let (op, params, label) = b.submit_continue().unwrap();
    assert_eq!(op.command(), "cherry_pick.continue");
    assert_eq!(label, "Finishing the cherry-pick");
    let outcome = done(run(&r, &mut s, &mut b, op, params));
    assert!(outcome.moved);
    assert!(outcome.history.can_undo);
    assert!(
        outcome.notice.starts_with("Cherry-pick finished: commit "),
        "{}",
        outcome.notice
    );
    refresh(&r, &mut s, &mut b, 4);
    assert!(b.info().is_none());
}

#[test]
fn revert_succeeds() {
    let (r, mut s) = opened();
    let mut b = MergeBox::default();
    let params = start(&mut b, Pick::Revert, 2);
    let outcome = done(run(&r, &mut s, &mut b, Op::Pick(Pick::Revert), params));
    assert!(outcome.moved);
    assert!(!outcome.show_changes);
    assert!(outcome.history.can_undo);
    assert!(
        outcome
            .notice
            .starts_with("Changes of “c2” reverted on “main”"),
        "{}",
        outcome.notice
    );
    assert_eq!(outcome.status.unwrap().operation, None);
    assert!(!b.busy());
}

#[test]
fn cherry_pick_abort_takes_two_steps() {
    let (r, mut s) = opened();
    let mut b = MergeBox::default();
    let params = start(&mut b, Pick::CherryPick, 3);
    done(run(&r, &mut s, &mut b, Op::Pick(Pick::CherryPick), params));
    refresh(&r, &mut s, &mut b, 1);

    let (op, params, _) = b.start_abort().unwrap();
    assert_eq!(op, Op::Abort(Kind::CherryPick));
    assert_eq!(params, json!({}));
    assert!(b.dialog().is_none());
    assert!(matches!(
        run(&r, &mut s, &mut b, op, params),
        Received::Confirm
    ));
    let Some(Dialog::Abort(Kind::CherryPick, Phase::Confirm { reason })) = b.dialog() else {
        panic!("abort confirmation");
    };
    assert!(reason.contains("a.txt"), "{reason}");

    let (op, params, label) = b.confirm_abort().unwrap();
    assert_eq!(params, json!({ "confirm": true }));
    assert_eq!(label, "Aborting the cherry-pick");
    let outcome = done(run(&r, &mut s, &mut b, op, params));
    assert!(
        outcome
            .notice
            .contains("refs/project/backup/cherry-pick-abort-"),
        "{}",
        outcome.notice
    );
    assert!(!outcome.moved);
    refresh(&r, &mut s, &mut b, 2);
    assert!(b.info().is_none());
}

#[test]
fn conflicted_revert_is_finished_from_the_panel() {
    let (r, mut s) = opened_on(4);
    let mut b = MergeBox::default();
    let params = start(&mut b, Pick::Revert, 2);
    let outcome = done(run(&r, &mut s, &mut b, Op::Pick(Pick::Revert), params));
    assert!(!outcome.moved);
    assert!(outcome.show_changes);
    assert!(
        outcome.notice.contains("“Continue revert…”"),
        "{}",
        outcome.notice
    );
    refresh(&r, &mut s, &mut b, 1);
    let info = b.info().expect("revert in progress");
    assert_eq!(info.operation, Some(Operation::Revert));
    assert_eq!(info.continue_command.as_deref(), Some("revert.continue"));
    assert_eq!(info.abort_command.as_deref(), Some("revert.abort"));

    r.run("index.stage", &mut s, json!({ "paths": ["a.txt"] }))
        .unwrap();
    refresh(&r, &mut s, &mut b, 2);
    assert!(b.open_continue());
    let (op, params, label) = b.submit_continue().unwrap();
    assert_eq!(op, Op::Continue(Kind::Revert));
    assert_eq!(label, "Finishing the revert");
    let outcome = done(run(&r, &mut s, &mut b, op, params));
    assert!(outcome.moved);
    assert!(outcome.history.can_undo);
    assert!(
        outcome.notice.starts_with("Revert finished: commit "),
        "{}",
        outcome.notice
    );
    refresh(&r, &mut s, &mut b, 3);
    assert!(b.info().is_none());
}

#[test]
fn revert_abort_takes_two_steps() {
    let (r, mut s) = opened_on(4);
    let mut b = MergeBox::default();
    let params = start(&mut b, Pick::Revert, 2);
    done(run(&r, &mut s, &mut b, Op::Pick(Pick::Revert), params));
    refresh(&r, &mut s, &mut b, 1);

    let (op, params, _) = b.start_abort().unwrap();
    assert_eq!(op, Op::Abort(Kind::Revert));
    assert!(matches!(
        run(&r, &mut s, &mut b, op, params),
        Received::Confirm
    ));
    let (op, params, label) = b.confirm_abort().unwrap();
    assert_eq!(params, json!({ "confirm": true }));
    assert_eq!(label, "Aborting the revert");
    let outcome = done(run(&r, &mut s, &mut b, op, params));
    assert!(
        outcome.notice.contains("refs/project/backup/revert-abort-"),
        "{}",
        outcome.notice
    );
    assert!(!outcome.moved);
    refresh(&r, &mut s, &mut b, 2);
    assert!(b.info().is_none());
}

#[test]
fn merge_commit_asks_for_the_parent() {
    let (r, mut s) = opened();
    let mut b = MergeBox::default();
    let parents = vec![
        (oid(2), "main".to_owned(), Some("c2".to_owned())),
        (oid(3), "other".to_owned(), None),
    ];
    assert!(
        b.start_pick(Pick::Revert, oid(3), "c3".into(), "main".into(), parents)
            .is_none()
    );
    let Some(Dialog::Mainline(dialog)) = b.dialog() else {
        panic!("parent dialog");
    };
    assert_eq!(dialog.mainline, 1);
    assert!(b.modal_open());
    b.set_mainline(3);
    b.set_mainline(2);
    let (op, params, _) = b.submit_mainline().unwrap();
    assert_eq!(op, Op::Pick(Pick::Revert));
    assert_eq!(params, json!({ "rev": oid(3).to_string(), "mainline": 2 }));
    assert!(b.dialog().is_none());

    // `c3` is not a merge: the engine refuses and the window opens.
    let Received::Failed { reload, .. } = run(&r, &mut s, &mut b, op, params) else {
        panic!("refused");
    };
    assert!(!reload);
    assert_eq!(b.failure().unwrap().title, "Revert not possible");
    assert!(!b.busy());
}
