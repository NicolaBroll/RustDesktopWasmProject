//! The merge dialog and the squash merge (S58) on the `MemoryBackend`: mode and message in the
//! parameters, the squash notices, the banner and the "Continue"/abort of a squash in progress.

use super::*;
use project_domain::{Change, ChangedFile, Commit, RepoPath, Signature};
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

/// Commit `n` with parent `parent` that brings `path` to the blob `100 + n` with `summary`.
fn commit(repo: &mut MemoryBackend, n: u64, parent: Option<u64>, summary: &str, path: &str) {
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
    // `a.txt` changed from the one of `c1`; the other files are added.
    let old = (path == "a.txt" && parent.is_some()).then(|| oid(101));
    repo.set_changes(
        oid(n),
        vec![ChangedFile {
            path: RepoPath::from(path),
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

/// `c1` adds `a.txt`; `main` on `c2` (a.txt “ours”); `other` on `c3` from `c1` (a.txt “theirs”:
/// conflict); `feature` on `c5` from `c2` (`c4` b.txt, `c5` c.txt: no conflict).
fn opened() -> (Registry, Session) {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    commit(&mut repo, 1, None, "base", "a.txt");
    commit(&mut repo, 2, Some(1), "ours", "a.txt");
    commit(&mut repo, 3, Some(1), "theirs", "a.txt");
    commit(&mut repo, 4, Some(2), "add b", "b.txt");
    commit(&mut repo, 5, Some(4), "add c", "c.txt");
    repo.set_branch("main", oid(2));
    repo.set_branch("other", oid(3));
    repo.set_branch("feature", oid(5));
    repo.set_head_branch("main");
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(opener));
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
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

/// Opens the dialog for `rev`, chooses `mode` and, if given, writes `message`; submit.
fn submit(b: &mut MergeBox, rev: &str, mode: Mode, message: Option<&str>) -> (Value, String) {
    assert!(b.open_start(rev.into(), rev.into(), "main".into()));
    assert!(b.modal_open());
    b.set_mode(mode);
    if let Some(message) = message {
        b.set_message(message.into());
    }
    let sent = b.submit_start().expect("started");
    assert!(b.dialog().is_none());
    assert!(b.busy());
    sent
}

#[test]
fn dialog_modes_become_parameters() {
    let mut b = MergeBox::default();
    assert!(b.open_start("x".into(), "x".into(), "main".into()));
    let Some(Dialog::Start(dialog)) = b.dialog() else {
        panic!("merge dialog");
    };
    assert_eq!(dialog.mode, Mode::MergeCommit);
    // A second dialog does not open over it; closing sends nothing.
    assert!(!b.open_start("y".into(), "y".into(), "main".into()));
    b.close();
    assert!(b.dialog().is_none() && !b.busy());

    let (params, label) = submit(&mut b, "x", Mode::MergeCommit, None);
    assert_eq!(params, json!({ "rev": "x", "no_ff": true }));
    assert_eq!(label, "Merge of “x” into “main”");

    let mut b = MergeBox::default();
    let (params, _) = submit(&mut b, "x", Mode::FastForwardOnly, None);
    assert_eq!(params, json!({ "rev": "x", "ff_only": true }));

    // The message written for a squash goes with it; for the other modes it does not.
    let mut b = MergeBox::default();
    let (params, label) = submit(&mut b, "x", Mode::Squash, Some("One commit\n\n"));
    assert_eq!(
        params,
        json!({ "rev": "x", "squash": true, "message": "One commit" })
    );
    assert_eq!(label, "Squash of “x” into “main”");

    let mut b = MergeBox::default();
    let (params, _) = submit(&mut b, "x", Mode::Squash, Some("  \n"));
    assert_eq!(params, json!({ "rev": "x", "squash": true }));

    let mut b = MergeBox::default();
    assert!(b.open_start("x".into(), "x".into(), "main".into()));
    b.set_mode(Mode::Squash);
    b.set_message("kept for the squash only".into());
    b.set_mode(Mode::MergeCommit);
    let (params, _) = b.submit_start().unwrap();
    assert_eq!(params, json!({ "rev": "x", "no_ff": true }));
}

#[test]
fn squash_creates_one_commit_with_its_notice() {
    let (r, mut s) = opened();
    let mut b = MergeBox::default();
    let (params, _) = submit(&mut b, "feature", Mode::Squash, None);
    let outcome = done(run(&r, &mut s, &mut b, Op::Start, params));
    assert!(outcome.moved);
    assert!(!outcome.show_changes);
    assert!(outcome.history.can_undo);
    assert!(
        outcome
            .notice
            .starts_with("“feature” squashed into “main” (2 commits): one commit ")
            && outcome.notice.contains("does not appear as merged"),
        "{}",
        outcome.notice
    );
    assert!(!b.busy());

    // A custom message reaches the commit.
    let (r, mut s) = opened();
    let mut b = MergeBox::default();
    let (params, _) = submit(&mut b, "feature", Mode::Squash, Some("Feature in one"));
    done(run(&r, &mut s, &mut b, Op::Start, params));
    let log = r.run("repo.log", &mut s, json!({ "limit": 1 })).unwrap();
    assert_eq!(log["commits"][0]["summary"], "Feature in one", "{log}");
}

#[test]
fn fast_forward_only_refused_on_diverged_branches() {
    let (r, mut s) = opened();
    let mut b = MergeBox::default();
    let (params, _) = submit(&mut b, "other", Mode::FastForwardOnly, None);
    let Received::Failed { reload, .. } = run(&r, &mut s, &mut b, Op::Start, params) else {
        panic!("refused");
    };
    assert!(!reload);
    assert_eq!(b.failure().unwrap().title, "Merge not possible");
}

#[test]
fn squash_conflict_banner_continue_and_abort() {
    let (r, mut s) = opened();
    let mut b = MergeBox::default();
    let (params, _) = submit(&mut b, "other", Mode::Squash, None);
    let outcome = done(run(&r, &mut s, &mut b, Op::Start, params));
    assert!(!outcome.moved);
    assert!(outcome.show_changes);
    assert!(
        outcome
            .notice
            .starts_with("Squash of “other” into “main” stopped: 1 conflicted file")
            && outcome.notice.contains("“Continue squash…”"),
        "{}",
        outcome.notice
    );

    // The banner: squash, with the merge commands under the squash name.
    refresh(&r, &mut s, &mut b, 1);
    let info = b.info().expect("squash in progress");
    assert_eq!(info.operation, Some(Operation::Squash));
    assert_eq!(title(info), "Squash in progress: “other” (theirs)");
    assert_eq!(Kind::resume_of(info), Some(Kind::Squash));
    assert_eq!(Kind::abort_of(info), Some(Kind::Squash));
    assert_eq!(Kind::Squash.continue_command(), "merge.continue");
    assert_eq!(
        Kind::Squash.abort_backup(),
        "refs/project/backup/merge-abort-…"
    );

    // Abort in two steps, with the merge command.
    let (op, params, label) = b.start_abort().unwrap();
    assert_eq!(op, Op::Abort(Kind::Squash));
    assert_eq!(op.command(), "merge.abort");
    assert_eq!(label, "Checking before aborting the squash");
    assert!(matches!(
        run(&r, &mut s, &mut b, op, params),
        Received::Confirm
    ));
    b.close();

    // Resolved and staged: "Continue squash" with the stored message, an ordinary commit.
    r.run("index.stage", &mut s, json!({ "paths": ["a.txt"] }))
        .unwrap();
    refresh(&r, &mut s, &mut b, 2);
    assert!(b.info().unwrap().ready);
    assert!(b.open_continue());
    let Some(Dialog::Continue(dialog)) = b.dialog() else {
        panic!("Continue dialog");
    };
    assert_eq!(dialog.kind, Kind::Squash);
    assert!(
        dialog.message.starts_with("Squashed branch 'other'"),
        "{}",
        dialog.message
    );
    assert_eq!(dialog.kind.commit_noun(), "commit");
    let (op, params, label) = b.submit_continue().unwrap();
    b.continue_started(Ticket::for_test(1));
    assert_eq!(op, Op::Continue(Kind::Squash));
    assert_eq!(label, "Finishing the squash");
    let outcome = done(run(&r, &mut s, &mut b, op, params));
    assert!(outcome.moved);
    assert!(
        outcome.notice.starts_with("Squash finished: commit ")
            && outcome.notice.contains("on “main”"),
        "{}",
        outcome.notice
    );
    refresh(&r, &mut s, &mut b, 3);
    assert!(b.info().is_none());
}

#[test]
fn squash_abort_confirmed() {
    let (r, mut s) = opened();
    let mut b = MergeBox::default();
    let (params, _) = submit(&mut b, "other", Mode::Squash, None);
    done(run(&r, &mut s, &mut b, Op::Start, params));
    refresh(&r, &mut s, &mut b, 1);
    let (op, params, _) = b.start_abort().unwrap();
    assert!(matches!(
        run(&r, &mut s, &mut b, op, params),
        Received::Confirm
    ));
    let (op, params, _) = b.confirm_abort().unwrap();
    assert_eq!(params, json!({ "confirm": true }));
    let outcome = done(run(&r, &mut s, &mut b, op, params));
    assert!(
        outcome.notice.starts_with(
            "Squash aborted: index and working tree saved first in refs/project/backup/merge-abort-"
        ),
        "{}",
        outcome.notice
    );
    assert_eq!(outcome.status.unwrap().operation, None);
}
