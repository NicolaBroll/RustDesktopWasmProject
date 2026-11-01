//! Blame and history from the UI on the `MemoryBackend`: run grouping, paging, renames, the
//! replaced requests that are cancelled and discarded.
//!
//! The repo: `c1` adds `a.txt` = one/two/three; `c2` changes `two` → `TWO`; `c3` renames
//! `a.txt` → `b.txt`; `c4` adds `four` to `b.txt` and the binary `bin.dat`. HEAD on `main` at
//! `c4`; in the worktree `b.txt` has a fifth, uncommitted line.

use super::*;
use crate::test_util::oid;
use project_domain::{Change, ChangedFile, Commit, Signature};
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use serde_json::json;
use std::sync::Arc;

const ZERO: Oid = Oid::from_bytes([0; Oid::LEN]);

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

/// A change: (path, original for a rename, content before, content after).
type Edit<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a str);

fn commit(repo: &mut MemoryBackend, n: u64, parent: Option<u64>, summary: &str, files: &[Edit]) {
    let changes = files
        .iter()
        .map(|&(path, original, old, new)| {
            let old_oid = old.map_or(ZERO, |c| blob(repo, c));
            let new_oid = blob(repo, new);
            let change = match (original, old) {
                (Some(_), _) => Change::Renamed,
                (None, Some(_)) => Change::Modified,
                (None, None) => Change::Added,
            };
            ChangedFile {
                path: RepoPath::from(path),
                change,
                original: original.map(RepoPath::from),
                score: original.map(|_| 100),
                old_mode: if old.is_some() { 0o100644 } else { 0 },
                new_mode: 0o100644,
                old_oid,
                new_oid,
            }
        })
        .collect();
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parent.map(oid).into_iter().collect(),
        author: sig(n),
        committer: sig(n),
        summary: summary.into(),
        body: String::new(),
    });
    repo.set_changes(oid(n), changes);
}

const V1: &str = "one\ntwo\nthree\n";
const V2: &str = "one\nTWO\nthree\n";
const V4: &str = "one\nTWO\nthree\nfour\n";

fn opened() -> (Registry, Session) {
    let mut repo = MemoryBackend::new();
    commit(&mut repo, 1, None, "add a", &[("a.txt", None, None, V1)]);
    commit(
        &mut repo,
        2,
        Some(1),
        "TWO",
        &[("a.txt", None, Some(V1), V2)],
    );
    let rename = [("b.txt", Some("a.txt"), Some(V2), V2)];
    commit(&mut repo, 3, Some(2), "rename", &rename);
    let four = [
        ("b.txt", None, Some(V2), V4),
        ("bin.dat", None, None, "x\0y\n"),
    ];
    commit(&mut repo, 4, Some(3), "four", &four);
    repo.set_branch("main", oid(4));
    repo.set_head_branch("main");
    repo.set_worktree_file("b.txt", format!("{V4}five\n"));
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(opener));
    registry
        .run("repo.open", &mut session, json!({ "path": "r" }))
        .unwrap();
    (registry, session)
}

/// Sends what the box wants like the executor (with ticket `seq`) and passes the answer back.
fn step(r: &Registry, s: &mut Session, b: &mut FileBox, edit: impl FnOnce(&mut Value)) -> bool {
    let mut want = b.want().expect("a request to send");
    b.sent(want.seq, Ticket::for_test(want.seq));
    edit(&mut want.params);
    let result = r
        .run(want.command, s, want.params)
        .map_err(|e| e.to_string());
    b.receive(want.seq, result)
}

fn blame_doc(b: &FileBox) -> &BlameDoc {
    match b.view().unwrap().blame() {
        Loaded::Ready(doc) => doc,
        other => panic!("blame not loaded: {other:?}"),
    }
}

/// For each run: (first line, length, summary of its commit or `None`).
fn summarize(doc: &BlameDoc) -> Vec<(usize, usize, Option<&str>)> {
    doc.runs
        .iter()
        .map(|r| {
            let summary = r.commit.map(|c| doc.blame.commits[c].summary.as_str());
            (r.start, r.len, summary)
        })
        .collect()
}

fn line(commit: Option<usize>) -> BlameLine {
    BlameLine {
        text: project_domain::ByteText::from_bytes(b"x".to_vec()),
        commit,
        orig_line: 1,
    }
}

#[test]
fn consecutive_lines_of_a_commit_form_one_run() {
    let lines = [Some(0), Some(0), None, None, Some(1), Some(0)].map(line);
    let got = runs(&lines);
    let want = [
        Run {
            start: 0,
            len: 2,
            commit: Some(0),
        },
        Run {
            start: 2,
            len: 2,
            commit: None,
        },
        Run {
            start: 4,
            len: 1,
            commit: Some(1),
        },
        Run {
            start: 5,
            len: 1,
            commit: Some(0),
        },
    ];
    assert_eq!(got, want);
    assert!(runs(&[]).is_empty());
}

#[test]
fn blame_of_the_worktree_groups_the_lines_and_marks_uncommitted_ones() {
    let (r, mut s) = opened();
    let mut b = FileBox::default();
    b.open(RepoPath::from("b.txt"), None, Mode::Blame);
    assert_eq!(b.view().unwrap().blame(), Loaded::Loading);
    assert!(step(&r, &mut s, &mut b, |_| {}));
    let doc = blame_doc(&b);
    assert_eq!(
        summarize(doc),
        [
            (0, 1, Some("add a")),
            (1, 1, Some("TWO")),
            (2, 1, Some("add a")),
            (3, 1, Some("four")),
            (4, 1, None),
        ]
    );
    assert_eq!(doc.run_at(4), 4);
    assert_eq!(doc.number_width, 1);
    // Read once: no new request on the following frames.
    assert_eq!(b.want(), None);
}

#[test]
fn blame_at_a_commit_and_the_binary_error() {
    let (r, mut s) = opened();
    let mut b = FileBox::default();
    b.open(RepoPath::from("a.txt"), Some(oid(2)), Mode::Blame);
    let want = b.want().unwrap();
    assert_eq!(want.command, BLAME);
    assert_eq!(want.params["rev"], json!(oid(2).to_string()));
    let result = r
        .run(want.command, &mut s, want.params)
        .map_err(|e| e.to_string());
    assert!(b.receive(want.seq, result));
    assert_eq!(
        summarize(blame_doc(&b)),
        [
            (0, 1, Some("add a")),
            (1, 1, Some("TWO")),
            (2, 1, Some("add a"))
        ]
    );

    b.open(RepoPath::from("bin.dat"), Some(oid(4)), Mode::Blame);
    assert!(step(&r, &mut s, &mut b, |_| {}));
    let Loaded::Failed(error) = b.view().unwrap().blame() else {
        panic!("a binary file is refused");
    };
    assert!(error.contains("bin.dat"), "{error}");
    // No new request until "Retry".
    assert_eq!(b.want(), None);
    b.retry();
    assert!(b.want().is_some());
}

#[test]
fn history_pages_with_load_more() {
    let (r, mut s) = opened();
    let mut b = FileBox::default();
    b.open(RepoPath::from("b.txt"), None, Mode::History);
    assert!(b.view().unwrap().log_first_loading());
    // Pages of two instead of `PAGE`.
    let small = |p: &mut Value| p["limit"] = json!(2);
    assert!(step(&r, &mut s, &mut b, small));
    let view = b.view().unwrap();
    let ids: Vec<_> = view.history.entries.iter().map(|e| e.commit.id).collect();
    assert_eq!(ids, [oid(4), oid(3)]);
    assert!(view.history.has_more);
    assert_eq!(b.want(), None);

    b.more();
    let want = b.want().unwrap();
    assert_eq!(want.params["skip"], json!(2));
    assert_eq!(want.params["follow"], json!(true));
    let mut params = want.params;
    small(&mut params);
    let result = r.run(LOG, &mut s, params).map_err(|e| e.to_string());
    assert!(b.receive(want.seq, result));
    let view = b.view().unwrap();
    let ids: Vec<_> = view.history.entries.iter().map(|e| e.commit.id).collect();
    assert_eq!(ids, [oid(4), oid(3), oid(2), oid(1)]);
    assert!(!view.history.has_more);
    // Nothing more to ask for.
    b.more();
    assert_eq!(b.want(), None);
    // The diff of an entry is that commit at the entry's path.
    let target = b.view().unwrap().entry_target(3).unwrap();
    assert_eq!(target, DiffTarget::commit(oid(1), RepoPath::from("a.txt")));
}

#[test]
fn renames_are_shown_and_follow_can_be_turned_off() {
    let (r, mut s) = opened();
    let mut b = FileBox::default();
    b.open(RepoPath::from("b.txt"), None, Mode::History);
    assert!(step(&r, &mut s, &mut b, |_| {}));
    let view = b.view().unwrap();
    let current = &view.path;
    let notes: Vec<_> = view
        .history
        .entries
        .iter()
        .map(|e| (entry_path(e, current), entry_origin(e)))
        .collect();
    assert_eq!(
        notes,
        [
            (None, None),
            (None, Some("renamed from a.txt".to_owned())),
            (Some("a.txt".to_owned()), None),
            (Some("a.txt".to_owned()), None),
        ]
    );

    // Without following, the history stops at the rename and is read again from the start.
    b.set_follow(false);
    let view = b.view().unwrap();
    assert!(view.history.entries.is_empty() && view.log_first_loading());
    let want = b.want().unwrap();
    assert_eq!(
        (want.params["follow"].clone(), want.params["skip"].clone()),
        (json!(false), json!(0))
    );
    let result = r.run(LOG, &mut s, want.params).map_err(|e| e.to_string());
    assert!(b.receive(want.seq, result));
    let ids: Vec<_> = b
        .view()
        .unwrap()
        .history
        .entries
        .iter()
        .map(|e| e.commit.id)
        .collect();
    assert_eq!(ids, [oid(4), oid(3)]);
}

#[test]
fn a_replaced_request_is_cancelled_and_its_answer_dropped() {
    let (r, mut s) = opened();
    let mut b = FileBox::default();
    b.open(RepoPath::from("b.txt"), None, Mode::Blame);
    let old = b.want().unwrap();
    b.sent(old.seq, Ticket::for_test(41));
    assert!(b.take_cancel().is_empty());

    // Another file replaces the blame in flight: its ticket is to cancel.
    b.open(RepoPath::from("a.txt"), Some(oid(2)), Mode::Blame);
    assert_eq!(b.take_cancel(), [Ticket::for_test(41)]);
    let new = b.want().unwrap();
    assert_ne!(new.seq, old.seq);
    let late = r
        .run(old.command, &mut s, old.params)
        .map_err(|e| e.to_string());
    assert!(!b.receive(old.seq, late));
    assert_eq!(b.view().unwrap().blame(), Loaded::Loading);

    // Toggling "Follow renames" cancels the page in flight.
    b.set_mode(Mode::History);
    let page = b.want().unwrap();
    b.sent(page.seq, Ticket::for_test(42));
    b.set_follow(false);
    assert_eq!(b.take_cancel(), [Ticket::for_test(42)]);
    assert!(!b.receive(page.seq, Ok(json!({ "commits": [], "has_more": false }))));

    // Closing cancels what is still in flight (the blame of a.txt) and drops its answer.
    b.sent(new.seq, Ticket::for_test(43));
    b.close();
    assert_eq!(b.take_cancel(), [Ticket::for_test(43)]);
    let result = r
        .run(new.command, &mut s, new.params)
        .map_err(|e| e.to_string());
    assert!(!b.receive(new.seq, result));
    assert!(b.view().is_none());
}

#[test]
fn reopening_the_same_file_keeps_what_was_read() {
    let (r, mut s) = opened();
    let mut b = FileBox::default();
    b.open(RepoPath::from("b.txt"), None, Mode::Blame);
    assert!(step(&r, &mut s, &mut b, |_| {}));
    b.open(RepoPath::from("b.txt"), None, Mode::History);
    assert!(matches!(b.view().unwrap().blame(), Loaded::Ready(_)));
    assert_eq!(b.want().unwrap().command, LOG);
}
