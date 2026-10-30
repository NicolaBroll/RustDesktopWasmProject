//! The search in the history on the `MemoryBackend`: debounce, stale answers discarded, pages of
//! ids, next/previous that stop at the ends, matches selected in the list with `seek`, Esc.
//!
//! The repo: a linear history of commits `1..=n` on `main`, commit `k` with summary `fix k` when
//! `k` is a multiple of `every`, `work k` otherwise; the most recent comes first in the list.

use super::*;
use crate::list::{CommitList, Seek};
use project_domain::{Commit, Signature};
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use serde_json::json;
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn opened(n: u64, every: u64) -> (Registry, Session) {
    let mut repo = MemoryBackend::new();
    for k in 1..=n {
        let sig = Signature {
            name: "Ada".into(),
            email: "ada@example.com".into(),
            time: 1_700_000_000 + i64::try_from(k).unwrap() * 60,
            offset_min: 0,
        };
        let summary = if k % every == 0 { "fix" } else { "work" };
        repo.insert_commit(Commit {
            id: oid(k),
            parents: if k > 1 { vec![oid(k - 1)] } else { Vec::new() },
            author: sig.clone(),
            committer: sig,
            summary: format!("{summary} {k}"),
            body: String::new(),
        });
        repo.set_changes(oid(k), Vec::new());
    }
    repo.set_branch("main", oid(n));
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

/// Sends the request due at `now` like the executor (ticket = seq) and returns it with its answer,
/// without passing it back to the box.
fn send(
    r: &Registry,
    s: &mut Session,
    b: &mut SearchBox,
    now: f64,
) -> (u64, Result<Value, String>) {
    let want = b.want(now).expect("a request to send");
    b.sent(want.seq, Ticket::for_test(want.seq));
    let result = r.run(COMMAND, s, want.params).map_err(|e| e.to_string());
    (want.seq, result)
}

/// Sends the request due at `now` and passes the answer back.
fn step(r: &Registry, s: &mut Session, b: &mut SearchBox, now: f64) -> Option<Oid> {
    let (seq, result) = send(r, s, b, now);
    b.receive(seq, result)
}

/// Types `text` at time 0 and runs the search once the debounce is over.
fn search(r: &Registry, s: &mut Session, b: &mut SearchBox, text: &str) -> Option<Oid> {
    b.open(Before::Nothing);
    b.edit(text.into(), 0.0);
    step(r, s, b, DEBOUNCE)
}

#[test]
fn typing_is_debounced_and_the_first_match_is_selected() {
    let (r, mut s) = opened(20, 5);
    let mut b = SearchBox::default();
    b.open(Before::Nothing);
    assert!(b.take_focus());
    assert!(!b.take_focus(), "only once");
    assert_eq!(b.state(), State::Idle);

    b.edit("fi".into(), 0.0);
    assert_eq!(b.want(0.1), None);
    b.edit("fix".into(), 0.2);
    assert_eq!(b.state(), State::Searching);
    assert_eq!(b.want(0.4), None, "the debounce starts again at each key");
    assert!((b.wait(0.4).unwrap() - 0.1).abs() < 1e-9);
    // The newest match first: commits 20, 15, 10, 5.
    assert_eq!(step(&r, &mut s, &mut b, 0.5), Some(oid(20)));
    assert_eq!(b.wait(0.5), None);
    assert_eq!(
        b.state(),
        State::Matches {
            current: Some(0),
            total: 4
        }
    );
    assert!(b.is_match(oid(15)));
    assert!(!b.is_match(oid(14)));
    assert_eq!(b.want(10.0), None, "nothing more to ask");

    // Literal and case-insensitive; an id prefix matches too.
    assert_eq!(search(&r, &mut s, &mut b, "FIX 1"), Some(oid(15)));
    let prefix = oid(7).to_string()[..10].to_owned();
    assert_eq!(search(&r, &mut s, &mut b, &prefix), Some(oid(7)));
    assert_eq!(search(&r, &mut s, &mut b, "fix.*"), None);
    assert_eq!(b.state(), State::NoMatches);
    assert!(!b.can_next() && !b.can_previous());
}

#[test]
fn texts_that_cannot_be_searched_are_never_sent() {
    let (r, mut s) = opened(10, 2);
    let mut b = SearchBox::default();
    search(&r, &mut s, &mut b, "fix");
    assert!(b.is_match(oid(10)));
    for text in ["", "a\nb", "a\0b"] {
        b.edit(text.into(), 1.0);
        assert_eq!(b.want(100.0), None, "{text:?}");
        assert!(!b.is_match(oid(10)), "the old matches are gone");
    }
    assert_eq!(b.state(), State::Idle);
}

#[test]
fn stale_answers_are_ignored_and_cancelled() {
    let (r, mut s) = opened(20, 5);
    let mut b = SearchBox::default();
    b.open(Before::Nothing);
    b.edit("work".into(), 0.0);
    let (old, old_result) = send(&r, &mut s, &mut b, 1.0);
    // The query changes while the answer is on its way: the old request is cancelled.
    b.edit("fix".into(), 1.1);
    let (new, new_result) = send(&r, &mut s, &mut b, 2.0);
    assert_eq!(b.take_cancel(), vec![Ticket::for_test(old)]);
    assert_eq!(b.receive(old, old_result), None);
    assert!(!b.is_match(oid(19)), "matches of the old query not shown");
    assert_eq!(b.state(), State::Searching);
    assert_eq!(b.receive(new, new_result.clone()), Some(oid(20)));
    assert_eq!(
        b.receive(new, new_result),
        None,
        "the same answer only once"
    );

    // Closed while a search is on its way: the answer is discarded.
    b.edit("work".into(), 3.0);
    let (seq, result) = send(&r, &mut s, &mut b, 4.0);
    b.close();
    assert_eq!(b.take_cancel(), vec![Ticket::for_test(seq)]);
    assert_eq!(b.receive(seq, result), None);
    assert_eq!(b.state(), State::Idle);

    // An error is shown in the bar.
    b.edit("fix".into(), 5.0);
    let want = b.want(6.0).unwrap();
    assert_eq!(b.receive(want.seq, Err("boom".into())), None);
    assert_eq!(b.state(), State::Failed("boom"));
}

#[test]
fn next_and_previous_stop_at_the_ends_and_load_pages_of_ids() {
    // 450 matches: three pages of ids (200, 200, 50).
    let (r, mut s) = opened(900, 2);
    let mut b = SearchBox::default();
    assert_eq!(search(&r, &mut s, &mut b, "fix"), Some(oid(900)));
    assert!(!b.can_previous());
    assert_eq!(b.previous(), None, "the first match stays");
    assert_eq!(
        b.state(),
        State::Matches {
            current: Some(0),
            total: 450
        }
    );
    let mut selected = vec![oid(900)];
    let mut pages = 1;
    let mut now = 1.0;
    while b.can_next() {
        match b.next() {
            Some(id) => selected.push(id),
            None => {
                // Past the loaded ids: the next page, with `skip`, and the move when it arrives.
                assert_eq!(b.next(), None, "one page at a time");
                now += 1.0;
                let want = b.want(now).unwrap();
                assert_eq!(want.params["skip"], json!(pages * PAGE));
                assert_eq!(want.params["limit"], json!(PAGE));
                let result = r
                    .run(COMMAND, &mut s, want.params)
                    .map_err(|e| e.to_string());
                let id = b
                    .receive(want.seq, result)
                    .expect("moves onto the new page");
                selected.push(id);
                pages += 1;
            }
        }
    }
    assert_eq!(pages, 3);
    let expected: Vec<Oid> = (1..=450).rev().map(|k| oid(k * 2)).collect();
    assert_eq!(selected, expected);
    assert_eq!(b.next(), None, "the last match stays");
    assert_eq!(b.want(now + 1.0), None);
    assert_eq!(
        b.state(),
        State::Matches {
            current: Some(449),
            total: 450
        }
    );
    assert_eq!(b.previous(), Some(oid(4)));
    assert_eq!(b.next(), Some(oid(2)));
}

#[test]
fn scrolling_past_the_last_loaded_match_asks_for_the_next_page() {
    let (r, mut s) = opened(900, 2);
    let mut b = SearchBox::default();
    search(&r, &mut s, &mut b, "fix");
    let last = b.last_if_more().unwrap();
    assert_eq!(last, oid(900 - 2 * (PAGE as u64 - 1)));
    assert!(!b.is_match(oid(900 - 2 * PAGE as u64)));
    b.load_more();
    assert_eq!(b.last_if_more(), None, "already asked");
    step(&r, &mut s, &mut b, 1.0);
    assert!(b.is_match(oid(900 - 2 * PAGE as u64)));
    assert_eq!(
        b.state(),
        State::Matches {
            current: Some(0),
            total: 450
        },
        "the selection did not move"
    );
}

#[test]
fn a_refresh_runs_the_query_again_and_keeps_the_current_match() {
    let (r, mut s) = opened(20, 5);
    let mut b = SearchBox::default();
    search(&r, &mut s, &mut b, "fix");
    assert_eq!(b.next(), Some(oid(15)));
    b.refresh();
    let (seq, result) = send(&r, &mut s, &mut b, 1.0);
    // Until the answer arrives the matches stay.
    assert!(b.is_match(oid(15)));
    assert_eq!(
        b.state(),
        State::Matches {
            current: Some(1),
            total: 4
        }
    );
    assert_eq!(b.receive(seq, result), None, "the selection does not move");
    assert_eq!(b.next(), Some(oid(10)));
}

#[test]
fn escape_clears_the_field_and_the_highlight() {
    let (r, mut s) = opened(20, 5);
    let mut b = SearchBox::default();
    search(&r, &mut s, &mut b, "fix");
    b.close();
    assert!(!b.is_open());
    assert_eq!(b.text(), "");
    assert!(!b.is_match(oid(20)));
    assert!(!b.can_next());
    assert_eq!(b.next(), None);
    assert_eq!(b.state(), State::Idle);
    // Opened again, it starts empty.
    b.open(Before::Nothing);
    assert_eq!(b.want(100.0), None);
}

#[test]
fn a_match_beyond_the_loaded_commits_is_selected_with_seek() {
    // 1200 commits, the list loads 500 at a time; matches every 400: 1200, 800, 400.
    let (r, mut s) = opened(1_200, 400);
    let mut list = CommitList::default();
    let first = list.reset();
    let page = r.run("repo.log", &mut s, first.params()).unwrap();
    let commits: Vec<Commit> = serde_json::from_value(page["commits"].clone()).unwrap();
    assert!(list.receive(first, commits, page["has_more"].as_bool().unwrap()));

    let mut b = SearchBox::default();
    assert_eq!(search(&r, &mut s, &mut b, "fix"), Some(oid(1_200)));
    assert_eq!(b.next(), Some(oid(800)));
    // The third match (commit 400, row 800) is in the second page of the log.
    let target = b.next().unwrap();
    assert_eq!(target, oid(400));
    let mut seek = list.seek(target);
    while let Seek::Loading(Some(request)) = seek {
        let page = r.run("repo.log", &mut s, request.params()).unwrap();
        let commits: Vec<Commit> = serde_json::from_value(page["commits"].clone()).unwrap();
        assert!(list.receive(request, commits, page["has_more"].as_bool().unwrap()));
        seek = list.seek(list.seeking().unwrap());
    }
    assert_eq!(seek, Seek::Selected);
    assert_eq!(list.selected_commit().unwrap().id, oid(400));
    assert_eq!(list.take_scroll_to(), Some(800));
}

#[test]
fn closing_without_choosing_gives_back_the_selection_from_before() {
    let (r, mut s) = opened(20, 5);
    let mut b = SearchBox::default();
    b.open(Before::Worktree);
    b.edit("fix".into(), 0.0);
    // The first match is selected while the bar is open...
    assert_eq!(step(&r, &mut s, &mut b, DEBOUNCE), Some(oid(20)));
    // ...Ctrl+F again does not replace what was selected before.
    b.open(Before::Commit(oid(20)));
    // Esc: the "Uncommitted changes" row comes back.
    assert_eq!(b.close(), Some(Before::Worktree));
    assert_eq!(b.close(), None, "already closed");
}

#[test]
fn enter_or_a_click_on_a_result_keeps_it() {
    let (r, mut s) = opened(20, 5);
    let mut b = SearchBox::default();
    b.open(Before::Commit(oid(3)));
    b.edit("fix".into(), 0.0);
    step(&r, &mut s, &mut b, DEBOUNCE);
    // Enter (or ↓) goes to a match: it stays selected after Esc.
    assert_eq!(b.next(), Some(oid(15)));
    assert_eq!(b.close(), None);
    // A click in the list while the bar is open.
    search(&r, &mut s, &mut b, "fix");
    b.keep();
    assert_eq!(b.close(), None);
    // Opened again after being kept: the new selection is remembered.
    b.open(Before::Stash(oid(7)));
    assert_eq!(b.close(), Some(Before::Stash(oid(7))));
}
