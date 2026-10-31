//! Tracking branches and upstream (S51): the plan of "Check out" on a remote branch, the choices
//! of the "Set upstream" window and the full paths with the sample repo through the `Registry`.

use super::*;
use crate::branch::NameDialog;
use project_domain::{Oid, Upstream};
use project_engine::{GitRead, LogQuery, MemoryBackend, MemoryOpener, Registry, Session};
use serde_json::json;
use std::sync::Arc;

fn r(name: &str, kind: RefKind, upstream: Option<&str>) -> RefInfo {
    RefInfo {
        name: name.to_owned(),
        kind,
        target: Oid::from_bytes([7; 20]),
        tag_object: None,
        upstream: upstream.map(|u| Upstream {
            name: u.to_owned(),
            ahead: Some(0),
            behind: Some(0),
        }),
    }
}

fn origin() -> Vec<String> {
    vec!["origin".to_owned()]
}

#[test]
fn the_plan_reuses_the_tracking_branch_or_creates_one() {
    let refs = vec![
        r(
            "refs/heads/mine",
            RefKind::LocalBranch,
            Some("refs/remotes/origin/feat/x"),
        ),
        r("refs/heads/main", RefKind::LocalBranch, None),
        r("refs/remotes/origin/feat/x", RefKind::RemoteBranch, None),
        r("refs/remotes/origin/main", RefKind::RemoteBranch, None),
        r("refs/remotes/origin/new/y", RefKind::RemoteBranch, None),
    ];
    let plan = |full| plan(full, &refs, &origin());
    // Already tracked, even under another name.
    assert_eq!(
        plan("refs/remotes/origin/feat/x"),
        Some(Plan::Checkout("mine".to_owned()))
    );
    // `main` exists but does not track it: another name is needed.
    assert_eq!(
        plan("refs/remotes/origin/main"),
        Some(Plan::Taken("main".to_owned()))
    );
    assert_eq!(
        plan("refs/remotes/origin/new/y"),
        Some(Plan::Create("new/y".to_owned()))
    );
    assert_eq!(plan("refs/remotes/origin/HEAD"), None);
    assert_eq!(plan("refs/heads/main"), None);
    // A remote whose name contains `/`: the longest one owns the ref.
    let remotes = vec!["a".to_owned(), "a/b".to_owned()];
    assert_eq!(
        super::plan("refs/remotes/a/b/c", &refs, &remotes),
        Some(Plan::Create("c".to_owned()))
    );
    // Without the remotes list: up to the first `/`.
    assert_eq!(
        super::plan("refs/remotes/origin/new/y", &refs, &[]),
        Some(Plan::Create("new/y".to_owned()))
    );
}

#[test]
fn the_window_offers_remote_branches_and_preselects_the_same_name() {
    let refs = vec![
        r("refs/heads/topic", RefKind::LocalBranch, None),
        r("refs/remotes/origin/HEAD", RefKind::RemoteBranch, None),
        r("refs/remotes/origin/main", RefKind::RemoteBranch, None),
        r("refs/remotes/origin/topic", RefKind::RemoteBranch, None),
        r("refs/remotes/gone/topic", RefKind::RemoteBranch, None),
        r("refs/tags/topic", RefKind::Tag, None),
    ];
    let choices = upstream_choices(&refs, &origin());
    assert_eq!(
        choices,
        ["refs/remotes/origin/main", "refs/remotes/origin/topic"]
    );
    assert_eq!(
        preselect("topic", None, &choices, &origin()).as_deref(),
        Some("refs/remotes/origin/topic")
    );
    let current = Some("refs/remotes/origin/main");
    assert_eq!(
        preselect("topic", current, &choices, &origin()).as_deref(),
        current
    );
    assert_eq!(preselect("other", None, &choices, &origin()), None);

    let mut b = BranchBox::default();
    assert!(b.open_upstream("topic".to_owned(), None, choices, None));
    assert!(b.modal_open());
    assert_eq!(b.submit_upstream(), None, "nothing picked");
    b.set_upstream_filter("MAI".to_owned());
    let Some(Dialog::Upstream(d)) = b.dialog() else {
        panic!()
    };
    assert_eq!(d.visible(), ["refs/remotes/origin/main"]);
    // Only one of the choices can be picked.
    b.pick_upstream("refs/remotes/origin/nope".to_owned());
    b.pick_upstream("refs/remotes/origin/main".to_owned());
    let (params, label) = b.submit_upstream().unwrap();
    assert_eq!(
        params,
        json!({ "name": "topic", "upstream": "refs/remotes/origin/main" })
    );
    assert_eq!(label, "Setting the upstream of “topic” to origin/main");
    b.upstream_started(Ticket::for_test(1));
    b.close();
    assert!(b.modal_open(), "not closed during the submit");
    let received = b.receive(Op::SetUpstream, Err("nope".to_owned()), None, None, None);
    assert!(matches!(received, Received::Inline), "{received:?}");
    let Some(Dialog::Upstream(d)) = b.dialog() else {
        panic!()
    };
    assert_eq!(d.error.as_deref(), Some("nope"));
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

fn refs(r: &Registry, s: &mut Session) -> Vec<RefInfo> {
    serde_json::from_value(r.run("repo.refs", s, json!({})).unwrap()).unwrap()
}

fn upstream_of(refs: &[RefInfo], name: &str) -> Option<String> {
    let full = format!("refs/heads/{name}");
    refs.iter()
        .find(|r| r.name == full)?
        .upstream
        .as_ref()
        .map(|u| u.name.clone())
}

/// The sample repo with `origin/feat/x`, a remote branch without a local one.
fn with_remote_only() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    let head = repo.log(&LogQuery::page(0, 1)).unwrap().remove(0);
    repo.set_remote_branch("origin/feat/x", head.id);
    repo
}

#[test]
fn check_out_a_remote_branch_creates_the_tracking_branch_then_switches() {
    let (r, mut s) = open(with_remote_only());
    let all = refs(&r, &mut s);
    let full = "refs/remotes/origin/feat/x";
    assert_eq!(
        plan(full, &all, &origin()),
        Some(Plan::Create("feat/x".to_owned()))
    );
    let mut b = BranchBox::default();
    let params = b.start_track("feat/x".to_owned(), full).unwrap();
    assert_eq!(
        params,
        json!({ "name": "feat/x", "start": full, "track": true })
    );
    assert!(!b.modal_open(), "no dialog");
    assert_eq!(b.start_unset("main".to_owned()), None, "one at a time");
    let outcome = done(send(&mut b, &r, &mut s, Op::Create, params));
    assert_eq!(outcome.then_checkout.as_deref(), Some("feat/x"));
    assert!(
        outcome.notice.contains("tracking origin/feat/x"),
        "{}",
        outcome.notice
    );
    let now = outcome.refs.unwrap();
    assert_eq!(upstream_of(&now, "feat/x").as_deref(), Some(full));
    // Again: the branch that tracks it is reused.
    assert_eq!(
        plan(full, &now, &origin()),
        Some(Plan::Checkout("feat/x".to_owned()))
    );
    // Undo removes branch and upstream.
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert!(
        !refs(&r, &mut s)
            .iter()
            .any(|i| i.name == "refs/heads/feat/x")
    );
}

#[test]
fn a_taken_name_opens_the_dialog_that_tracks_and_checks_out() {
    let (r, mut s) = open(MemoryBackend::sample());
    let all = refs(&r, &mut s);
    let mut b = BranchBox::default();
    // `main` exists and tracks `origin/main`: reused. Untrack it to have a taken name.
    assert_eq!(
        plan("refs/remotes/origin/main", &all, &origin()),
        Some(Plan::Checkout("main".to_owned()))
    );
    let params = b.start_unset("main".to_owned()).unwrap();
    assert_eq!(params, json!({ "name": "main", "upstream": null }));
    let outcome = done(send(&mut b, &r, &mut s, Op::SetUpstream, params));
    assert!(
        outcome.notice.contains("no longer tracks origin/main"),
        "{}",
        outcome.notice
    );
    let all = outcome.refs.unwrap();
    assert_eq!(upstream_of(&all, "main"), None);
    assert_eq!(
        plan("refs/remotes/origin/main", &all, &origin()),
        Some(Plan::Taken("main".to_owned()))
    );

    let info = all
        .iter()
        .find(|i| i.name == "refs/remotes/origin/main")
        .unwrap();
    assert!(b.open_taken(Start::from_ref(info).0, "main".to_owned()));
    let Some(Dialog::Name(NameDialog {
        track,
        checkout_after,
        error,
        ..
    })) = b.dialog()
    else {
        panic!()
    };
    assert!(*track && *checkout_after);
    assert!(error.as_deref().unwrap().contains("already exists"));
    b.set_name("main-2".to_owned());
    let (op, params, _) = b.submit_name().unwrap();
    assert_eq!(
        params,
        json!({ "name": "main-2", "start": "refs/remotes/origin/main", "track": true })
    );
    b.name_started(Ticket::for_test(2));
    let outcome = done(send(&mut b, &r, &mut s, op, params));
    assert_eq!(outcome.then_checkout.as_deref(), Some("main-2"));
    assert_eq!(
        upstream_of(&outcome.refs.unwrap(), "main-2").as_deref(),
        Some("refs/remotes/origin/main")
    );

    // Unchecked "Track": a plain branch at the commit, not checked out.
    let (start, _) = Start::from_ref(info);
    let target = start.target;
    assert!(b.open_create(start, "plain".to_owned()));
    b.set_track(false);
    let (op, params, _) = b.submit_name().unwrap();
    assert_eq!(
        params,
        json!({ "name": "plain", "start": target.to_string() })
    );
    b.name_started(Ticket::for_test(3));
    let outcome = done(send(&mut b, &r, &mut s, op, params));
    assert_eq!(outcome.then_checkout, None);
    assert_eq!(upstream_of(&outcome.refs.unwrap(), "plain"), None);
}

#[test]
fn set_upstream_through_the_window_and_undo() {
    let (r, mut s) = open(MemoryBackend::sample());
    let all = refs(&r, &mut s);
    let choices = upstream_choices(&all, &origin());
    let current = upstream_of(&all, "feature/graph");
    let selected = preselect("feature/graph", current.as_deref(), &choices, &origin());
    assert_eq!(selected, current, "its own upstream first");
    let mut b = BranchBox::default();
    assert!(b.open_upstream("feature/graph".to_owned(), current, choices, selected));
    assert_eq!(b.submit_upstream(), None, "the current one changes nothing");
    b.pick_upstream("refs/remotes/origin/main".to_owned());
    let (params, _) = b.submit_upstream().unwrap();
    b.upstream_started(Ticket::for_test(1));
    let outcome = done(send(&mut b, &r, &mut s, Op::SetUpstream, params));
    assert!(b.dialog().is_none());
    assert!(
        outcome.notice.contains("now tracks origin/main"),
        "{}",
        outcome.notice
    );
    assert!(outcome.history.can_undo);
    assert_eq!(
        upstream_of(&outcome.refs.unwrap(), "feature/graph").as_deref(),
        Some("refs/remotes/origin/main")
    );
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(
        upstream_of(&refs(&r, &mut s), "feature/graph").as_deref(),
        Some("refs/remotes/origin/feature/graph")
    );
    // Removing an upstream of a branch that does not exist: refused, not reloaded.
    let params = b.start_unset("nope".to_owned()).unwrap();
    let received = send(&mut b, &r, &mut s, Op::SetUpstream, params);
    let Received::Failed { message, reload } = received else {
        panic!("{received:?}")
    };
    assert!(
        message.starts_with("Cannot remove the upstream of “nope”"),
        "{message}"
    );
    assert!(!reload);
}
