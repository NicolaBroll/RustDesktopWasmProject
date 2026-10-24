//! The commit-graph notice: when it is shown (threshold, "Not now", graph already there) and
//! `repo.optimize` on the `MemoryBackend` (success hides it, refusal keeps it with the text).

use super::*;
use project_domain::{Commit, Signature};
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use std::sync::Arc;

fn t(n: u64) -> Ticket {
    Ticket::for_test(n)
}

/// A box whose first page took `took`.
fn timed(commit_graph: bool, took: Duration) -> OptimizeBox {
    let mut b = OptimizeBox::new(commit_graph);
    let start = Instant::now();
    b.log_submitted(t(1), start);
    b.log_received(t(1), start + took, true);
    b
}

#[test]
fn offered_only_without_graph_after_a_slow_first_page() {
    let slow = SLOW_FIRST_PAGE + Duration::from_millis(1);
    assert!(should_offer(false, Some(slow), false));
    // At the threshold, faster, not arrived yet, graph present, dismissed: no.
    assert!(!should_offer(false, Some(SLOW_FIRST_PAGE), false));
    assert!(!should_offer(false, Some(Duration::from_millis(40)), false));
    assert!(!should_offer(false, None, false));
    assert!(!should_offer(true, Some(slow), false));
    assert!(!should_offer(false, Some(slow), true));
    assert!(timed(false, Duration::from_secs(14)).shown());
    assert!(!timed(true, Duration::from_secs(14)).shown());
    assert!(!timed(false, Duration::from_millis(45)).shown());
}

#[test]
fn only_the_first_page_is_timed_and_a_failed_one_is_timed_again() {
    let start = Instant::now();
    let mut b = OptimizeBox::new(false);
    b.log_submitted(t(1), start);
    // Another page (or another request) answering does not count.
    b.log_received(t(2), start + Duration::from_secs(5), true);
    assert_eq!(b.first_page(), None);
    b.log_received(t(1), start + Duration::from_secs(1), false);
    assert_eq!(b.first_page(), None);
    b.log_submitted(t(3), start + Duration::from_secs(2));
    b.log_received(t(3), start + Duration::from_secs(4), true);
    assert_eq!(b.first_page(), Some(Duration::from_secs(2)));
    // A later reload (Refresh) does not change the measure.
    b.log_submitted(t(4), start + Duration::from_secs(10));
    b.log_received(t(4), start + Duration::from_secs(10), true);
    assert_eq!(b.first_page(), Some(Duration::from_secs(2)));
    assert!(b.shown());
}

#[test]
fn not_now_hides_it_but_not_while_running() {
    let mut b = timed(false, Duration::from_secs(3));
    b.started(t(9));
    b.dismiss();
    assert!(b.shown());
    assert_eq!(b.request(), None, "already running");
    assert_eq!(b.cancel(), Some(t(9)));
    assert_eq!(b.cancel(), None, "only once");
    let kind = Some(ErrorKind::Cancelled);
    let received = b.receive(t(9), Err("cancelled".into()), kind, None);
    assert_eq!(received, Received::Cancelled);
    assert!(b.shown() && b.error().is_none());
    b.dismiss();
    assert!(!b.shown());
    assert_eq!(b.request(), None);
}

fn repo() -> MemoryBackend {
    let mut repo = MemoryBackend::new();
    let ada = Signature {
        name: "Ada".into(),
        email: "ada@example.com".into(),
        time: 1_700_000_000,
        offset_min: 0,
    };
    repo.insert_commit(Commit {
        id: MemoryBackend::fake_oid(1),
        parents: vec![],
        author: ada.clone(),
        committer: ada,
        summary: "c1".into(),
        body: String::new(),
    });
    repo.set_branch("main", MemoryBackend::fake_oid(1));
    repo.set_head_branch("main");
    repo
}

/// Opens `repo`, reads `commit_graph` like the UI and runs `repo.optimize` like the executor.
fn optimize(repo: MemoryBackend) -> (OptimizeBox, Received) {
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(opener));
    let opened = registry
        .run("repo.open", &mut session, json!({ "path": "r" }))
        .unwrap();
    let mut b = timed(
        opened["commit_graph"] == json!(true),
        Duration::from_secs(3),
    );
    assert!(b.shown(), "no graph in the MemoryBackend at first");
    let params = b.request().expect("offered");
    b.started(t(5));
    let result = registry.run(COMMAND, &mut session, params);
    let (kind, detail) = match &result {
        Err(e) => (Some(ErrorKind::of(e)), ErrorKind::detail(e)),
        Ok(_) => (None, None),
    };
    let received = b.receive(t(5), result.map_err(|e| e.text()), kind, detail);
    (b, received)
}

#[test]
fn written_graph_hides_the_notice() {
    let (b, received) = optimize(repo());
    assert_eq!(received, Received::Done);
    assert!(!b.shown() && b.running().is_none() && b.error().is_none());
}

#[test]
fn refusal_keeps_the_notice_with_the_engine_text() {
    let mut repo = repo();
    repo.set_config("core.commitGraph", &["false"]);
    let (b, received) = optimize(repo);
    let Received::Failed(message) = received else {
        panic!("expected Failed, got {received:?}");
    };
    assert!(message.contains("core.commitGraph"), "{message}");
    assert_eq!(b.error(), Some(message.as_str()));
    assert!(b.shown() && b.request().is_some(), "can be retried");
}
