use super::*;
use crate::merge::{MergeBox, OPERATION};
use crate::test_util::oid;
use project_domain::{Change, ChangedFile, Commit, Oid, Signature};
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use std::sync::Arc;

fn sig(n: u64) -> Signature {
    Signature {
        name: "Ada".into(),
        email: "ada@example.com".into(),
        time: 1_700_000_000 + i64::try_from(n).unwrap() * 60,
        offset_min: 0,
    }
}

/// Commit `n` with parent `parent` that takes `a.txt` from blob `old` to blob `new`, whose
/// content is `text`.
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

/// `main` on `c2` and `other` on `c3`, both from `c1`: `merge.start` of `other` stops in
/// conflict on `a.txt`.
fn merging() -> (Registry, Session) {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    commit(&mut repo, 1, None, "base\n");
    commit(&mut repo, 2, Some(1), "ours\n");
    commit(&mut repo, 3, Some(1), "theirs\n");
    repo.set_branch("main", oid(2));
    repo.set_branch("other", oid(3));
    repo.set_head_branch("main");
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(opener));
    registry
        .run("repo.open", &mut session, json!({ "path": "r" }))
        .unwrap();
    let started = registry
        .run("merge.start", &mut session, json!({ "rev": "other" }))
        .unwrap();
    assert_eq!(started["in_progress"], true, "{started}");
    (registry, session)
}

/// Opens the editor on `path` and passes the `conflict.show` answer.
fn open(r: &Registry, s: &mut Session, b: &mut ConflictBox, path: &str) {
    let (params, seq) = b.open(RepoPath::from(path)).unwrap();
    b.receive_show(seq, r.run(SHOW, s, params).map_err(|e| e.text()));
}

/// Runs `conflict.resolve` like the executor.
fn send(r: &Registry, s: &mut Session, b: &mut ConflictBox, params: Value) -> Received {
    let result = r.run(RESOLVE, s, params);
    let kind = result.as_ref().err().map(ErrorKind::of);
    b.receive_resolve(result.map_err(|e| e.text()), kind)
}

fn loaded(b: &ConflictBox) -> &Loaded {
    b.editor().and_then(Editor::loaded).expect("versions read")
}

/// An editor already read with this worktree text, without the engine.
fn with_text(text: &str) -> ConflictBox {
    let mut b = ConflictBox::default();
    let (_, seq) = b.open(RepoPath::from("f.txt")).unwrap();
    let version = |t: &str| json!({ "text": t, "binary": false, "size": t.len() });
    b.receive_show(
        seq,
        Ok(json!({
            "path": "f.txt", "kind": "BothModified",
            "base": version("b\n"), "ours": version("o\n"), "theirs": version("t\n"),
            "worktree": version(text), "marker_size": 7, "digest": "d",
        })),
    );
    b
}

const TWO: &str = "start\n<<<<<<< HEAD\no1\n=======\nt1\n>>>>>>> feature\nmiddle\n<<<<<<< HEAD\no2\n\
                   =======\nt2\n>>>>>>> feature\nend\n";

#[test]
fn conflict_resolved_from_the_ui_and_merge_ready() {
    let (r, mut s) = merging();
    let mut b = ConflictBox::default();
    open(&r, &mut s, &mut b, "a.txt");
    let l = loaded(&b);
    assert_eq!(l.base, Column::Text(vec!["base".into()]));
    assert_eq!(l.ours, Column::Text(vec!["ours".into()]));
    assert_eq!(l.theirs, Column::Text(vec!["theirs".into()]));
    assert_eq!(l.ours_label.as_deref(), Some("HEAD"));
    let blocks = l.blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!((blocks[0].line, blocks[0].has_base), (1, false));
    // The same file is not reread (the edits stay).
    assert!(b.open(RepoPath::from("a.txt")).is_none());

    // "Both", then one more line by hand.
    assert!(b.choose(0, Choice::OursThenTheirs));
    assert_eq!(loaded(&b).result(), Some("ours\ntheirs\n"));
    b.set_result("ours\ntheirs\nend\n".into());
    assert!(loaded(&b).blocks().is_empty());

    let (params, label) = b.resolve().expect("without markers it is sent right away");
    b.started(Ticket::for_test(1));
    assert_eq!(params["content"], "ours\ntheirs\nend\n");
    assert_eq!(label, "Resolving the conflict in a.txt");
    assert!(b.busy());
    let Received::Done { status, notice } = send(&r, &mut s, &mut b, params) else {
        panic!("resolved");
    };
    assert!(!status.has_conflicts());
    assert!(notice.contains("Conflict in a.txt resolved"), "{notice}");
    assert!(b.editor().is_none());

    // The box rereads with the new state: no conflicts, "Continue" becomes active.
    let mut merge = MergeBox::default();
    assert!(merge.want_info(status.operation, 1));
    merge.receive_info(r.run(OPERATION, &mut s, json!({})).map_err(|e| e.text()));
    let info = merge.info().expect("merge in progress");
    assert!(info.ready);
    assert!(info.conflicts.is_empty());
}

#[test]
fn file_changed_after_reading_is_reread() {
    let (r, mut s) = merging();
    let mut b = ConflictBox::default();
    open(&r, &mut s, &mut b, "a.txt");
    assert!(b.choose(0, Choice::Theirs));
    let (mut params, _) = b.resolve().unwrap();
    b.started(Ticket::for_test(1));
    params["digest"] = json!("0".repeat(32));
    let Received::Failed { reread, .. } = send(&r, &mut s, &mut b, params) else {
        panic!("StaleDiff");
    };
    let (params, seq) = reread.expect("the file is reread");
    let editor = b.editor().unwrap();
    assert!(matches!(editor.load, Load::Loading));
    assert!(editor.error.as_deref().unwrap().contains("changed"));
    b.receive_show(seq, r.run(SHOW, &mut s, params).map_err(|e| e.text()));
    assert_eq!(loaded(&b).blocks().len(), 1);
}

#[test]
fn remaining_markers_ask_for_two_step_confirmation() {
    let mut b = with_text(TWO);
    assert_eq!(loaded(&b).blocks().len(), 2);
    assert_eq!(loaded(&b).theirs_label.as_deref(), Some("feature"));
    assert!(b.choose(1, Choice::Ours));
    assert_eq!(
        loaded(&b).result(),
        Some("start\n<<<<<<< HEAD\no1\n=======\nt1\n>>>>>>> feature\nmiddle\no2\nend\n")
    );
    let blocks = loaded(&b).blocks();
    assert_eq!((blocks.len(), blocks[0].line), (1, 2));

    // First step: nothing to send, the confirmation opens.
    assert!(b.resolve().is_none());
    assert_eq!(b.editor().unwrap().pending, Some(Pending::Markers(1)));
    // Giving up sends nothing.
    b.cancel();
    assert_eq!(b.editor().unwrap().pending, None);
    assert!(b.confirm().is_none());
    // Second step: the text with the markers is sent.
    assert!(b.resolve().is_none());
    let (params, _) = b.confirm().expect("confirmed");
    b.started(Ticket::for_test(1));
    assert!(params["content"].as_str().unwrap().contains("<<<<<<< HEAD"));
    assert_eq!(params["digest"], "d");
    assert!(b.busy());
}

#[test]
fn unparsable_text_has_no_per_block_buttons() {
    // Markers broken by hand: the text stays editable, but without blocks.
    let mut b = with_text(TWO);
    b.set_result("<<<<<<< HEAD\nx\n".into());
    let l = loaded(&b);
    assert!(l.blocks().is_empty());
    assert!(l.parse_error().is_some());
    assert_eq!(l.result(), Some("<<<<<<< HEAD\nx\n"));
    assert!(!b.choose(0, Choice::Ours));
    assert!(!b.choose_all(Choice::Theirs));
    assert!(b.editor().unwrap().error.is_some());
    // Without parsed blocks it is sent without confirmation (the warning is about the markers found).
    assert!(b.resolve().is_some());
    b.started(Ticket::for_test(1));

    // The worktree file already broken: no editable result, only whole sides.
    let mut b = with_text("<<<<<<< HEAD\nonly ours\n");
    let l = loaded(&b);
    assert!(l.result().is_none());
    assert!(l.blocks().is_empty());
    assert!(b.resolve().is_none());
    let (params, label) = b.use_side(Pick::Theirs).unwrap();
    b.started(Ticket::for_test(1));
    assert_eq!(params["content"], "t\n");
    assert!(label.contains("their"), "{label}");
}

#[test]
fn all_theirs_and_base() {
    let mut b = with_text(
        "<<<<<<< ours\na\n||||||| base\nb\n=======\nc\n>>>>>>> theirs\nx\n<<<<<<< ours\nd\n\
         ||||||| base\ne\n=======\nf\n>>>>>>> theirs\n",
    );
    assert!(loaded(&b).blocks().iter().all(|b| b.has_base));
    assert!(b.choose(1, Choice::Base));
    assert!(b.choose_all(Choice::Theirs));
    assert_eq!(loaded(&b).result(), Some("c\nx\ne\n"));
}

#[test]
fn deletion_asks_for_confirmation() {
    let mut b = ConflictBox::default();
    let (_, seq) = b.open(RepoPath::from("f.txt")).unwrap();
    b.receive_show(
        seq,
        Ok(json!({
            "path": "f.txt", "kind": "DeletedByUs", "base": null, "ours": null,
            "theirs": { "text": "t\n", "binary": false, "size": 2 },
            "worktree": null, "marker_size": 7, "digest": "d",
        })),
    );
    let l = loaded(&b);
    assert!(l.result().is_none() && l.can_delete());
    assert_eq!(l.ours, Column::Missing);
    assert!(b.use_side(Pick::Ours).is_none());
    b.delete();
    assert_eq!(b.editor().unwrap().pending, Some(Pending::Delete));
    let (params, _) = b.confirm().unwrap();
    b.started(Ticket::for_test(1));
    assert_eq!(
        params,
        json!({ "path": "f.txt", "digest": "d", "delete": true })
    );
}

#[test]
fn old_answers_dropped_and_closing_on_new_state() {
    let mut b = ConflictBox::default();
    let (_, old) = b.open(RepoPath::from("a.txt")).unwrap();
    let (_, _) = b.restart().unwrap();
    b.receive_show(old, Err("old".into()));
    assert!(matches!(b.editor().unwrap().load, Load::Loading));

    // Resolved elsewhere (stage from the changes area): the editor closes.
    let (r, mut s) = merging();
    let mut b = ConflictBox::default();
    open(&r, &mut s, &mut b, "a.txt");
    let status: Status =
        serde_json::from_value(r.run("repo.status", &mut s, json!({})).unwrap()).unwrap();
    b.follow_status(&status);
    assert!(b.editor().is_some());
    r.run("index.stage", &mut s, json!({ "paths": ["a.txt"] }))
        .unwrap();
    let status: Status =
        serde_json::from_value(r.run("repo.status", &mut s, json!({})).unwrap()).unwrap();
    b.follow_status(&status);
    assert!(b.editor().is_none());
}
