//! A rebase stopped on a conflict, finished or aborted from the panel on the `MemoryBackend`:
//! "Continue" without a message (it may stop again on the next commit) and two-step abort.

use super::*;
use project_domain::{Change, ChangedFile, Commit, RepoPath, Signature};
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

/// Commit `n` ("c<n>") with parent `parent` that brings `a.txt` to the blob with content `text`.
fn commit(repo: &mut MemoryBackend, n: u64, parent: Option<u64>, summary: &str) {
    let old = parent.map(|p| oid(100 + p));
    repo.set_blob(oid(100 + n), format!("{summary}\n"));
    let sig = Signature {
        name: "Ada".into(),
        email: "ada@example.com".into(),
        time: 1_700_000_000 + i64::try_from(n).unwrap() * 60,
        offset_min: 0,
    };
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parent.map(oid).into_iter().collect(),
        author: sig.clone(),
        committer: sig,
        summary: summary.into(),
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

/// `main`: c1 "base" → c2 "one" → c3 "two", all on `a.txt`. The rebase onto c1 with c3 before c2
/// stops on `a.txt` at c3, and again at c2 once the markers are staged.
fn stopped() -> (Registry, Session) {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    commit(&mut repo, 1, None, "base");
    commit(&mut repo, 2, Some(1), "one");
    commit(&mut repo, 3, Some(2), "two");
    repo.set_branch("main", oid(3));
    repo.set_head_branch("main");
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(opener));
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
    let pick = |n: u64| json!({ "action": "pick", "commit": oid(n).to_string() });
    let out = r
        .run(
            "rebase.start",
            &mut s,
            json!({ "base": oid(1).to_string(), "steps": [pick(3), pick(2)] }),
        )
        .unwrap();
    assert_eq!(out["in_progress"], true, "{out}");
    (r, s)
}

fn run(r: &Registry, s: &mut Session, b: &mut MergeBox, op: Op, params: Value) -> Received {
    let result = r.run(op.command(), s, params);
    let (kind, detail) = match &result {
        Err(e) => (Some(ErrorKind::of(e)), ErrorKind::detail(e)),
        Ok(_) => (None, None),
    };
    b.receive(op, result.map_err(|e| e.text()), kind, detail)
}

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

#[test]
fn continue_without_message_stops_again_then_finishes() {
    let (r, mut s) = stopped();
    let mut b = MergeBox::default();
    refresh(&r, &mut s, &mut b, 1);
    let info = b.info().expect("rebase in progress");
    assert_eq!(info.operation, Some(Operation::Rebase));
    assert_eq!(info.continue_command.as_deref(), Some("rebase.continue"));
    assert_eq!(info.abort_command.as_deref(), Some("rebase.abort"));
    assert_eq!(info.conflicts, vec![RepoPath::from("a.txt")]);

    // No message to write: the dialog can be sent as is, with `{}`; with conflicts it is refused
    // and the error stays in the dialog.
    assert!(b.open_continue());
    let Some(Dialog::Continue(dialog)) = b.dialog() else {
        panic!("Continue dialog");
    };
    assert_eq!(dialog.kind, Kind::Rebase);
    assert!(!dialog.is_blank());
    let (op, params, label) = b.submit_continue().unwrap();
    b.continue_started(Ticket::for_test(1));
    assert_eq!(op, Op::Continue(Kind::Rebase));
    assert_eq!(params, json!({}));
    assert_eq!(label, "Finishing the rebase");
    assert!(matches!(
        run(&r, &mut s, &mut b, op, params),
        Received::Inline
    ));
    let Some(Dialog::Continue(dialog)) = b.dialog() else {
        panic!("the dialog stays open");
    };
    assert!(dialog.error.as_deref().unwrap().contains("a.txt"));
    b.close();

    // Staged with the markers: c2 stops again on `a.txt`.
    r.run("index.stage", &mut s, json!({ "paths": ["a.txt"] }))
        .unwrap();
    refresh(&r, &mut s, &mut b, 2);
    assert!(b.info().unwrap().ready);
    assert!(b.open_continue());
    let (op, params, _) = b.submit_continue().unwrap();
    b.continue_started(Ticket::for_test(1));
    let outcome = done(run(&r, &mut s, &mut b, op, params));
    assert!(outcome.show_changes);
    assert!(outcome.moved);
    assert!(
        outcome
            .notice
            .starts_with("Rebase stopped again on the next commit: 1 conflicted file"),
        "{}",
        outcome.notice
    );
    assert_eq!(
        outcome.status.as_ref().and_then(|s| s.operation),
        Some(Operation::Rebase)
    );
    assert!(!outcome.history.can_undo);
    assert!(b.dialog().is_none());
    refresh(&r, &mut s, &mut b, 3);
    assert_eq!(b.info().unwrap().conflicts, vec![RepoPath::from("a.txt")]);

    r.run("index.stage", &mut s, json!({ "paths": ["a.txt"] }))
        .unwrap();
    refresh(&r, &mut s, &mut b, 4);
    assert!(b.open_continue());
    let (op, params, _) = b.submit_continue().unwrap();
    b.continue_started(Ticket::for_test(1));
    let outcome = done(run(&r, &mut s, &mut b, op, params));
    assert!(!outcome.show_changes);
    assert!(outcome.moved);
    assert!(
        outcome.notice.starts_with("Rebase of “main” finished")
            && outcome.notice.contains("refs/project/backup/rebase-"),
        "{}",
        outcome.notice
    );
    assert_eq!(outcome.warning, None);
    assert!(outcome.history.can_undo);
    assert_eq!(outcome.status.as_ref().unwrap().operation, None);
    refresh(&r, &mut s, &mut b, 5);
    assert!(b.info().is_none());
}

#[test]
fn abort_in_two_steps_puts_the_branch_back() {
    let (r, mut s) = stopped();
    let mut b = MergeBox::default();
    refresh(&r, &mut s, &mut b, 1);

    let (op, params, _) = b.start_abort().unwrap();
    assert_eq!(op, Op::Abort(Kind::Rebase));
    assert_eq!(params, json!({}));
    assert!(matches!(
        run(&r, &mut s, &mut b, op, params),
        Received::Confirm
    ));
    let Some(Dialog::Abort(Kind::Rebase, Phase::Confirm { reason })) = b.dialog() else {
        panic!("abort confirmation");
    };
    assert!(reason.contains("a.txt"), "{reason}");
    assert!(
        Kind::Rebase
            .abort_backup()
            .starts_with("refs/project/backup/rebase-abort-")
    );

    let (op, params, label) = b.confirm_abort().unwrap();
    assert_eq!(params, json!({ "confirm": true }));
    assert_eq!(label, "Aborting the rebase");
    let outcome = done(run(&r, &mut s, &mut b, op, params));
    assert!(
        outcome
            .notice
            .starts_with("Rebase aborted: index and working tree saved first in refs/project/backup/rebase-abort-"),
        "{}",
        outcome.notice
    );
    assert!(outcome.moved);
    let status = outcome.status.unwrap();
    assert_eq!(status.operation, None);
    assert_eq!(
        status.head,
        Head::Branch {
            name: "main".into(),
            target: Some(oid(3))
        }
    );
    refresh(&r, &mut s, &mut b, 2);
    assert!(b.info().is_none());
}
