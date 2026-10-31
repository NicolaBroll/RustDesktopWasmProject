//! Checking out a commit (S55): the confirmation, the parameters, the refusals shared with
//! `branch.checkout` and the full paths with the sample repo through the `Registry`.

use super::*;
use crate::ErrorKind;
use crate::branch::{Received, Refused};
use project_domain::{Commit, Signature};
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use serde_json::json;
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn open(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(opener));
    registry
        .run("repo.open", &mut session, json!({ "path": "r" }))
        .unwrap();
    (registry, session)
}

fn clean_sample() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    repo.set_entries(Vec::new());
    repo
}

/// Sends `params` to `op` and passes the answer to the box, as the UI does.
fn send(b: &mut BranchBox, r: &Registry, s: &mut Session, op: Op, params: Value) -> Received {
    match r.run(op.command(), s, params) {
        Ok(value) => b.receive(op, Ok(value), None, None, None),
        Err(e) => b.receive(
            op,
            Err(e.text()),
            Some(ErrorKind::of(&e)),
            ErrorKind::detail(&e),
            ErrorKind::expected(&e),
        ),
    }
}

fn done(r: Received) -> Outcome {
    match r {
        Received::Done(outcome) => *outcome,
        other => panic!("{other:?}"),
    }
}

fn head(s: &Session) -> Head {
    s.repo().unwrap().head().unwrap()
}

#[test]
fn the_confirmation_starts_the_checkout_of_the_full_id() {
    let mut b = BranchBox::default();
    let id = oid(10);
    assert!(b.open_detach(id));
    assert!(b.modal_open());
    // One dialog at a time, no checkout under it.
    assert!(!b.open_detach(oid(9)));
    assert_eq!(b.start_checkout_commit(oid(9)), None);
    b.set_dont_ask(true);
    assert_eq!(
        b.dialog(),
        Some(&Dialog::Detach(DetachDialog { id, dont_ask: true }))
    );
    // Cancel sends nothing.
    b.close();
    assert!(b.dialog().is_none());
    assert_eq!(b.confirm_detach(), None);

    b.open_detach(id);
    assert_eq!(b.confirm_detach(), Some(json!({ "rev": id.to_string() })));
    assert!(b.dialog().is_none(), "closed while the checkout runs");
    assert!(!b.open_detach(id), "busy until the answer");
}

#[test]
fn conflicts_and_refusals_are_those_of_branch_checkout() {
    let mut b = BranchBox::default();
    b.start_checkout_commit(oid(10)).unwrap();
    let r = b.receive(
        Op::CheckoutCommit,
        Err("refused".to_owned()),
        Some(ErrorKind::CheckoutConflict),
        Some("README.md".to_owned()),
        None,
    );
    assert!(matches!(r, Received::Refused));
    assert_eq!(
        b.dialog(),
        Some(&Dialog::Refused(Refused {
            name: format!("commit {}", oid(10).short()),
            files: vec!["README.md".to_owned()],
        }))
    );
    b.close();

    b.start_checkout_commit(oid(10)).unwrap();
    let r = b.receive(
        Op::CheckoutCommit,
        Err("rebase in progress".to_owned()),
        Some(ErrorKind::NotAllowed),
        None,
        None,
    );
    let Received::Failed { message, reload } = r else {
        panic!("{r:?}")
    };
    assert!(message.contains("rebase in progress"), "{message}");
    assert!(!reload);
}

/// With the sample repo: refused because of local changes, then detached on a clean worktree,
/// already there, "Create branch here" and back to a branch.
#[test]
fn checkout_a_commit_with_the_sample_repo() {
    let (r, mut s) = open(MemoryBackend::sample());
    let mut b = BranchBox::default();
    let params = b.start_checkout_commit(oid(9)).unwrap();
    assert!(matches!(
        send(&mut b, &r, &mut s, Op::CheckoutCommit, params),
        Received::Refused
    ));
    b.close();

    let (r, mut s) = open(clean_sample());
    let params = b.start_checkout_commit(oid(10)).unwrap();
    let out = done(send(&mut b, &r, &mut s, Op::CheckoutCommit, params));
    assert_eq!(out.op, Op::CheckoutCommit);
    assert_eq!(out.status.unwrap().head, Head::Detached(oid(10)));
    assert!(
        out.notice
            .starts_with(&format!("HEAD detached at {}", oid(10).short())),
        "{}",
        out.notice
    );
    assert!(out.history.can_undo);

    let params = b.start_checkout_commit(oid(10)).unwrap();
    let out = done(send(&mut b, &r, &mut s, Op::CheckoutCommit, params));
    assert!(out.notice.contains("already detached"), "{}", out.notice);

    // "Create branch here…": no `start`, then checked out.
    assert!(b.open_here(oid(10)));
    b.set_name("try".to_owned());
    let (op, params, _) = b.submit_name().unwrap();
    assert_eq!(params, json!({ "name": "try" }));
    b.name_started(crate::Ticket::for_test(1));
    let out = done(send(&mut b, &r, &mut s, op, params));
    assert_eq!(out.then_checkout.as_deref(), Some("try"));
    let params = b.start_checkout("try".to_owned()).unwrap();
    done(send(&mut b, &r, &mut s, Op::Checkout, params));
    assert!(matches!(head(&s), Head::Branch { name, .. } if name == "try"));
}

/// Leaving a commit no ref reaches: the notice names the commits saved and the backup, for
/// `commit.checkout` and `branch.checkout` alike.
#[test]
fn the_notice_names_the_commits_saved() {
    let mut repo = clean_sample();
    let sig = Signature {
        name: "Ada".into(),
        email: "ada@example.com".into(),
        time: 1_800_000_000,
        offset_min: 0,
    };
    repo.insert_commit(Commit {
        id: oid(100),
        parents: vec![oid(12)],
        author: sig.clone(),
        committer: sig,
        summary: "loose".into(),
        body: String::new(),
    });
    let (r, mut s) = open(repo);
    let mut b = BranchBox::default();
    let params = b.start_checkout_commit(oid(100)).unwrap();
    done(send(&mut b, &r, &mut s, Op::CheckoutCommit, params));

    let params = b.start_checkout_commit(oid(10)).unwrap();
    let out = done(send(&mut b, &r, &mut s, Op::CheckoutCommit, params));
    assert!(
        out.notice.ends_with(
            "; warning: 1 commit not on any branch was saved as refs/project/backup/detached-1"
        ),
        "{}",
        out.notice
    );

    let params = b.start_checkout_commit(oid(100)).unwrap();
    done(send(&mut b, &r, &mut s, Op::CheckoutCommit, params));
    let params = b.start_checkout("main".to_owned()).unwrap();
    let out = done(send(&mut b, &r, &mut s, Op::Checkout, params));
    assert!(
        out.notice
            .contains("1 commit not on any branch was saved as"),
        "{}",
        out.notice
    );
}

#[test]
fn a_backup_without_its_warning_is_still_named() {
    let text = backup_text(&[], Some("refs/project/backup/detached-9"));
    assert_eq!(
        text,
        "; warning: the commits not on any branch were saved as refs/project/backup/detached-9"
    );
    assert_eq!(backup_text(&[], None), "");
}
