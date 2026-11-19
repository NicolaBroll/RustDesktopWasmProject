//! "Stash and retry" (U68): the state machine as data (the retry follows only a successful
//! stash submitted with "Stash and …", once, with the parameters that were refused) and the whole
//! flow on a tab with the `MemoryBackend`.

use super::*;
use crate::discard::{self, UndoOp};
use crate::executor::{ErrorKind, Executor};
use crate::stash::{self, Dialog, Op, Received, StashBox};
use crate::test_util::oid;
use crate::{Action, Tab};
use project_domain::{Change, ChangedFile, Commit, Oid, Operation, RepoPath, Signature};
use project_engine::{Capabilities, MemoryBackend, MemoryOpener, Platform, Registry, Session};
use serde_json::json;
use std::sync::Arc;

fn main_at(n: u64) -> Head {
    Head::Branch {
        name: "main".to_owned(),
        target: Some(oid(n)),
    }
}

/// A checkout of `experiment` refused while the tab showed `main` at commit 11.
fn refused() -> Retry {
    let params = json!({ "name": "experiment" });
    let checkout = branch::Checkout {
        name: "experiment".to_owned(),
        params: params.clone(),
    };
    let mut retry = Retry::new(
        params,
        "Checking out “experiment”".to_owned(),
        "Stash and check out “experiment”".to_owned(),
        Route::Checkout(branch::Op::Checkout, checkout),
    );
    retry.seen_at(&main_at(11));
    retry
}

fn status(head: Head, operation: Option<Operation>) -> Status {
    Status {
        head,
        upstream: None,
        entries: Vec::new(),
        operation,
    }
}

#[test]
fn the_retry_is_sent_only_where_the_refusal_came_from() {
    let retry = refused();
    assert_eq!(retry.command(), "branch.checkout");
    assert_eq!(retry.params(), &json!({ "name": "experiment" }));
    assert_eq!(retry.check(Some(&status(main_at(11), None))), Ok(()));

    // Another branch, or the same branch on another commit: dropped.
    let other = Head::Branch {
        name: "other".to_owned(),
        target: Some(oid(11)),
    };
    assert_eq!(retry.check(Some(&status(other, None))), Err(Dropped::Head));
    assert_eq!(
        retry.check(Some(&status(main_at(12), None))),
        Err(Dropped::Head)
    );
    assert_eq!(
        retry.check(Some(&status(Head::Detached(oid(11)), None))),
        Err(Dropped::Head)
    );
    // An operation in progress, even on the same commit.
    for operation in [Operation::Merge, Operation::Rebase, Operation::CherryPick] {
        assert_eq!(
            retry.check(Some(&status(main_at(11), Some(operation)))),
            Err(Dropped::Operation)
        );
    }
    // The status did not come back with the stash: nothing to compare with.
    assert_eq!(retry.check(None), Err(Dropped::Unknown));
}

#[test]
fn head_is_the_one_noted_first_and_without_it_nothing_is_sent() {
    let Retry {
        params,
        label,
        button,
        route,
        ..
    } = refused();
    let mut retry = Retry::new(params, label, button, route);
    assert_eq!(
        retry.check(Some(&status(main_at(11), None))),
        Err(Dropped::Unknown)
    );
    retry.seen_at(&main_at(11));
    // A later answer (the tab re-read the status meanwhile) does not move it.
    retry.seen_at(&main_at(12));
    assert_eq!(retry.check(Some(&status(main_at(11), None))), Ok(()));
    assert_eq!(
        retry.check(Some(&status(main_at(12), None))),
        Err(Dropped::Head)
    );
}

#[test]
fn a_dropped_retry_says_where_the_changes_are() {
    let text = Dropped::Head.message("Pull of “main”", "stash@{0}");
    assert_eq!(
        text,
        "Changes stashed in stash@{0}. Not started again: Pull of “main” (the current branch \
         or its commit is no longer the one it was refused on); start it again yourself"
    );
    for dropped in [Dropped::Operation, Dropped::Unknown, Dropped::Busy] {
        let text = dropped.message("Pull", "stash@{0}");
        let start = "Changes stashed in stash@{0}. Not started again: Pull (";
        assert!(text.starts_with(start), "{text}");
    }
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

/// Runs the submitted `stash.push` like the executor and gives the answer to the box.
fn run_push(r: &Registry, s: &mut Session, b: &mut StashBox, params: Value) -> Received {
    b.push_started(Ticket::for_test(1));
    let result = r.run(Op::Push.command(), s, params);
    let kind = result.as_ref().err().map(ErrorKind::of);
    let detail = result.as_ref().err().and_then(ErrorKind::detail);
    b.receive(Op::Push, result.map_err(|e| e.text()), kind, detail)
}

fn offered(b: &StashBox) -> Option<&Retry> {
    match b.dialog() {
        Some(Dialog::Push(dialog)) => dialog.retry.as_ref(),
        _ => None,
    }
}

#[test]
fn the_retry_follows_a_successful_stash_once_with_the_same_parameters() {
    let (r, mut s) = opened(MemoryBackend::sample());
    let mut b = StashBox::default();
    assert!(b.open_push(Some(refused())));
    assert_eq!(offered(&b), Some(&refused()));
    let (params, _) = b.submit_push_and_retry().expect("submitted");
    let Received::Done(outcome) = run_push(&r, &mut s, &mut b, params) else {
        panic!("stashed");
    };
    let (retry, stash) = outcome.retry.expect("the retry");
    // What was refused, untouched: same command, same parameters, no `confirm` or `force`.
    assert_eq!(retry, refused());
    assert_eq!(retry.command(), "branch.checkout");
    assert_eq!(retry.params(), &json!({ "name": "experiment" }));
    assert_eq!(stash, "stash@{0}");
    // The dialog closed and took the retry with it: a second answer hands nothing back.
    assert!(b.dialog().is_none());
    let again = b.receive(Op::Push, Ok(json!({})), None, None);
    assert!(matches!(again, Received::Failed { .. }), "{again:?}");
}

#[test]
fn a_later_stash_from_the_toolbar_never_retries() {
    let (r, mut s) = opened(MemoryBackend::sample());
    let mut b = StashBox::default();
    // Opened from a refusal, then closed: the retry is gone with the dialog.
    assert!(b.open_push(Some(refused())));
    b.close();
    assert!(b.dialog().is_none());
    assert!(b.open_push(None));
    assert_eq!(offered(&b), None);
    // "Stash and …" is not there, and cannot be sent.
    assert_eq!(b.submit_push_and_retry(), None);
    let (params, _) = b.submit_push().expect("submitted");
    let Received::Done(outcome) = run_push(&r, &mut s, &mut b, params) else {
        panic!("stashed");
    };
    assert_eq!(outcome.retry, None);
}

#[test]
fn the_plain_stash_of_a_dialog_opened_from_a_refusal_sends_nothing_else() {
    let (r, mut s) = opened(MemoryBackend::sample());
    let mut b = StashBox::default();
    assert!(b.open_push(Some(refused())));
    let (params, _) = b.submit_push().expect("submitted");
    let Received::Done(outcome) = run_push(&r, &mut s, &mut b, params) else {
        panic!("stashed");
    };
    assert_eq!(outcome.retry, None);
}

#[test]
fn no_retry_after_a_failed_stash() {
    let mut b = StashBox::default();
    assert!(b.open_push(Some(refused())));
    b.submit_push_and_retry().expect("submitted");
    b.push_started(Ticket::for_test(1));
    // git failed: the dialog closes with the error, the retry is dropped with it.
    let received = b.receive(
        Op::Push,
        Err("fatal: unable to write".to_owned()),
        Some(ErrorKind::Git),
        None,
    );
    assert!(matches!(received, Received::Failed { reload: true, .. }));
    assert!(b.dialog().is_none());
    // A stash from the toolbar afterwards is a plain one.
    assert!(b.open_push(None));
    assert_eq!(b.submit_push_and_retry(), None);
}

#[test]
fn no_retry_when_there_is_nothing_to_stash() {
    let mut repo = MemoryBackend::sample();
    repo.set_entries(Vec::new());
    let (r, mut s) = opened(repo);
    let mut b = StashBox::default();
    assert!(b.open_push(Some(refused())));
    let (params, _) = b.submit_push_and_retry().expect("submitted");
    // The engine refuses: the error stays in the dialog and nothing is handed back.
    let received = run_push(&r, &mut s, &mut b, params);
    assert!(matches!(received, Received::Inline), "{received:?}");
    let Some(Dialog::Push(dialog)) = b.dialog() else {
        panic!("the dialog stays");
    };
    assert!(dialog.error.is_some());
    // Closing it drops the retry.
    b.close();
    assert!(b.open_push(None));
    assert_eq!(offered(&b), None);
}

/// Jobs run inside `spawn`.
struct Inline;

impl Platform for Inline {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn pick_folder(&self, on_done: project_engine::OnPicked) {
        on_done(Ok(None));
    }
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        job();
    }
}

/// A tab with `repo` open and loaded.
fn tab_with(repo: MemoryBackend) -> Tab {
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let executor = Executor::new(
        Session::new(Arc::new(opener)),
        Registry::builtin(),
        Arc::new(Inline),
        Arc::new(|| {}),
    );
    let mut tab = Tab::new(0, executor);
    tab.open("r".to_owned());
    settle(&mut tab);
    tab
}

fn settle(tab: &mut Tab) {
    while tab.executor.is_busy() {
        tab.receive();
    }
}

/// Applies `action` and waits for its answers, then notes the failures like a frame does.
fn act(tab: &mut Tab, action: Action) {
    tab.apply(action);
    settle(tab);
    repo_mut(tab).queue_failures();
}

fn repo(tab: &Tab) -> &RepoView {
    tab.repo.as_ref().expect("repo")
}

fn repo_mut(tab: &mut Tab) -> &mut RepoView {
    tab.repo.as_mut().expect("repo")
}

fn checkout(name: &str) -> Action {
    Action::Branch(branch::Action::Checkout(name.to_owned()))
}

fn stash_action(action: stash::Action) -> Action {
    Action::Stash(action)
}

fn undo() -> Action {
    Action::Discard(discard::Action::Undo(UndoOp::Undo))
}

fn stashes(tab: &Tab) -> usize {
    repo(tab).stash.list().map_or(0, <[_]>::len)
}

fn branch_name(tab: &Tab) -> Option<&str> {
    match &repo(tab).head {
        Head::Branch { name, .. } => Some(name),
        Head::Detached(_) => None,
    }
}

/// The retry the stash dialog on screen offers.
fn dialog_retry(tab: &Tab) -> Option<&Retry> {
    offered(&repo(tab).stash)
}

#[test]
fn a_refused_checkout_goes_through_after_the_stash_and_undo_twice_goes_back() {
    // The sample repo has local changes to `README.md`, which `feature/graph` changes too.
    let mut tab = tab_with(MemoryBackend::sample());
    let before = repo(&tab).status.clone();
    act(&mut tab, checkout("feature/graph"));
    let (source, failure) = repo(&tab).failure().expect("the refusal");
    assert_eq!(source, Source::Checkout);
    assert!(failure.stash && failure.retry.is_some());

    // "Stash…" in the failure dialog: it closes, the stash dialog offers the checkout.
    act(&mut tab, stash_action(stash::Action::OpenAfter(source)));
    assert!(repo(&tab).failure().is_none());
    let retry = dialog_retry(&tab).expect("offered");
    assert_eq!(retry.button(), "Stash and check out “feature/graph”");
    assert_eq!(retry.command(), "branch.checkout");
    assert_eq!(retry.params(), &json!({ "name": "feature/graph" }));

    // "Stash and check out": the stash, then the checkout, with nothing in between.
    act(&mut tab, stash_action(stash::Action::SubmitAndRetry));
    assert!(tab.error.is_empty(), "{}", tab.error);
    assert_eq!(branch_name(&tab), Some("feature/graph"));
    assert_eq!(stashes(&tab), 1, "the stash stays: nothing pops it");
    assert!(repo(&tab).failure().is_none() && repo(&tab).stash.dialog().is_none());
    // The status bar tells both things.
    assert!(
        tab.notice.starts_with("Changes stashed in stash@{0}. "),
        "{}",
        tab.notice
    );
    assert!(tab.notice.contains("feature/graph"), "{}", tab.notice);

    // Two journal entries: Undo goes back to `main`, Undo again brings the changes back.
    act(&mut tab, undo());
    assert_eq!(branch_name(&tab), Some("main"));
    assert_eq!(stashes(&tab), 1);
    act(&mut tab, undo());
    assert_eq!(stashes(&tab), 0);
    assert_eq!(repo(&tab).status, before);
}

#[test]
fn the_plain_stash_and_the_toolbar_stash_send_nothing_after() {
    // "Stash" (the second button) in a dialog opened from a refusal.
    let mut tab = tab_with(MemoryBackend::sample());
    act(&mut tab, checkout("feature/graph"));
    act(
        &mut tab,
        stash_action(stash::Action::OpenAfter(Source::Checkout)),
    );
    assert!(dialog_retry(&tab).is_some());
    act(&mut tab, stash_action(stash::Action::Submit));
    assert_eq!(stashes(&tab), 1);
    assert_eq!(branch_name(&tab), Some("main"));
    assert!(!tab.notice.is_empty());

    // "Stash…" from the toolbar after a refusal that was closed: no retry, even asked for.
    let mut tab = tab_with(MemoryBackend::sample());
    act(&mut tab, checkout("feature/graph"));
    act(&mut tab, Source::Checkout.dismiss());
    act(&mut tab, stash_action(stash::Action::Open));
    assert!(repo(&tab).stash.dialog().is_some());
    assert!(dialog_retry(&tab).is_none());
    act(&mut tab, stash_action(stash::Action::SubmitAndRetry));
    assert_eq!(stashes(&tab), 0, "nothing was sent");
    act(&mut tab, stash_action(stash::Action::Submit));
    assert_eq!(stashes(&tab), 1);
    assert_eq!(branch_name(&tab), Some("main"));
}

#[test]
fn closing_the_stash_dialog_forgets_the_refused_command() {
    let mut tab = tab_with(MemoryBackend::sample());
    act(&mut tab, checkout("feature/graph"));
    act(
        &mut tab,
        stash_action(stash::Action::OpenAfter(Source::Checkout)),
    );
    assert!(dialog_retry(&tab).is_some());
    act(&mut tab, stash_action(stash::Action::CloseDialog));
    assert!(repo(&tab).stash.dialog().is_none());
    // Neither the failure nor a later stash dialog has it any more.
    assert!(repo(&tab).failure().is_none());
    act(&mut tab, stash_action(stash::Action::Open));
    assert!(dialog_retry(&tab).is_none());
    act(&mut tab, stash_action(stash::Action::Submit));
    assert_eq!((stashes(&tab), branch_name(&tab)), (1, Some("main")));
}

#[test]
fn the_retry_is_dropped_when_head_is_not_the_one_of_the_refusal() {
    let mut tab = tab_with(MemoryBackend::sample());
    // The tab shows another commit than the repo is on when the refusal arrives (HEAD moved
    // from outside and nothing re-read it yet): what it showed is what the user decided on.
    repo_mut(&mut tab).head = main_at(999);
    act(&mut tab, checkout("feature/graph"));
    act(
        &mut tab,
        stash_action(stash::Action::OpenAfter(Source::Checkout)),
    );
    assert!(dialog_retry(&tab).is_some());
    act(&mut tab, stash_action(stash::Action::SubmitAndRetry));
    // The stash is made, the checkout is not sent, and the status bar says so.
    assert_eq!(stashes(&tab), 1);
    assert_eq!(branch_name(&tab), Some("main"));
    assert!(
        tab.notice.starts_with(
            "Changes stashed in stash@{0}. Not started again: Checking out “feature/graph” \
             (the current branch or its commit is no longer"
        ),
        "{}",
        tab.notice
    );
    assert!(repo(&tab).failure().is_none());
}

#[test]
fn the_retry_is_dropped_when_an_operation_is_in_progress() {
    let mut tab = tab_with(MemoryBackend::sample());
    act(&mut tab, checkout("feature/graph"));
    act(
        &mut tab,
        stash_action(stash::Action::OpenAfter(Source::Checkout)),
    );
    let retry = dialog_retry(&tab).expect("offered").clone();
    // The status that came back with the stash says a merge is in progress.
    let generation = tab.generation;
    let view = repo_mut(&mut tab);
    view.stash.close();
    let mut status = view.status.clone().expect("status");
    status.operation = Some(Operation::Merge);
    view.set_status(status);
    tab.retry_after_stash(generation, retry.clone(), "stash@{0}".to_owned(), true);
    assert!(!tab.executor.is_busy(), "nothing was sent");
    assert!(
        tab.notice.contains("an operation is in progress"),
        "{}",
        tab.notice
    );

    // The status did not come back: the state cannot be checked.
    tab.notice.clear();
    tab.retry_after_stash(generation, retry.clone(), "stash@{0}".to_owned(), false);
    assert!(!tab.executor.is_busy());
    assert!(
        tab.notice.contains("could not be checked"),
        "{}",
        tab.notice
    );

    // Another repo was opened in the tab meanwhile: dropped without a word.
    tab.notice.clear();
    tab.retry_after_stash(generation + 1, retry, "stash@{0}".to_owned(), true);
    assert!(!tab.executor.is_busy());
    assert!(tab.notice.is_empty());
}

fn who() -> Signature {
    Signature {
        name: "Carla".into(),
        email: "carla@example.com".into(),
        time: 1_700_000_000 + 20 * 3_600,
        offset_min: 0,
    }
}

/// The commit `n` on `parent`, which adds `file` if given.
fn add_commit(repo: &mut MemoryBackend, n: u64, parent: u64, file: Option<&str>) -> Commit {
    let commit = Commit {
        id: oid(n),
        parents: vec![oid(parent)],
        author: who(),
        committer: who(),
        summary: format!("Commit {n}"),
        body: String::new(),
    };
    if let Some(file) = file {
        repo.set_blob(oid(5_000 + n), "from the commit\n");
        repo.set_changes(
            oid(n),
            vec![ChangedFile {
                path: RepoPath::from(file),
                change: Change::Added,
                original: None,
                score: None,
                old_mode: 0,
                new_mode: 0o100644,
                old_oid: Oid::from_bytes([0; Oid::LEN]),
                new_oid: oid(5_000 + n),
            }],
        );
    }
    commit
}

#[test]
fn a_refused_pull_goes_through_after_the_stash() {
    // `origin/main` has a commit `main` does not have, and `main` one the remote lacks: the
    // pull is a merge, refused for the local changes of the sample repo.
    let mut backend = MemoryBackend::sample();
    let commit = add_commit(&mut backend, 20, 11, None);
    backend.insert_remote_commit("origin", commit);
    backend.set_branch_on_remote("origin", "main", oid(20));
    let mut tab = tab_with(backend);
    let before = repo(&tab).head.clone();

    act(&mut tab, Action::Sync(sync::Action::Pull(None, None)));
    let (source, failure) = repo(&tab).failure().expect("the refusal");
    assert_eq!(source, Source::Sync);
    assert!(failure.stash && failure.retry.is_some(), "{failure:?}");

    act(&mut tab, stash_action(stash::Action::OpenAfter(source)));
    let retry = dialog_retry(&tab).expect("offered");
    assert_eq!(retry.button(), "Stash and pull");
    assert_eq!(retry.command(), "remote.pull");
    // The branch was pinned when the pull was first sent: sent again by name.
    assert_eq!(retry.params(), &json!({ "branch": "main" }));

    act(&mut tab, stash_action(stash::Action::SubmitAndRetry));
    assert!(repo(&tab).failure().is_none(), "{:?}", repo(&tab).failure());
    assert_eq!(stashes(&tab), 1);
    assert_ne!(repo(&tab).head, before, "the pull moved main");
    assert_eq!(branch_name(&tab), Some("main"));
    assert!(
        tab.notice.starts_with("Changes stashed in stash@{0}. "),
        "{}",
        tab.notice
    );

    // Undo twice: the branch back where it was, then the changes back in the files.
    act(&mut tab, undo());
    assert_eq!(repo(&tab).head, before);
    act(&mut tab, undo());
    assert_eq!(stashes(&tab), 0);
    assert!(repo(&tab).has_changes());
}

#[test]
fn a_push_is_never_offered_again() {
    // A push is not refused for local changes; were it ever, nothing would be recorded.
    let mut b = sync::SyncBox::default();
    let request = sync::Request::push(Some("main".to_owned()));
    b.started(Ticket::for_test(1), request);
    b.receive(
        Ticket::for_test(1),
        Err("refused".to_owned()),
        Some(ErrorKind::LocalChanges),
        Some("a.txt".to_owned()),
        None,
        &main_at(11),
    );
    let failure = b.failure_mut().expect("the failure");
    assert!(failure.stash);
    assert_eq!(failure.retry, None);
}

#[test]
fn a_refused_merge_goes_through_after_the_stash() {
    // `topic` forks from an old commit: merging it is a merge commit, refused for the local
    // changes of the sample repo.
    let mut backend = MemoryBackend::sample();
    let commit = add_commit(&mut backend, 30, 11, None);
    backend.insert_commit(commit);
    backend.set_branch("topic", oid(30));
    let mut tab = tab_with(backend);
    let before = repo(&tab).head.clone();

    let merge = merge::Action::Merge {
        rev: "refs/heads/topic".to_owned(),
        what: "topic".to_owned(),
    };
    act(&mut tab, Action::Merge(merge));
    let (source, failure) = repo(&tab).failure().expect("the refusal");
    assert_eq!(source, Source::Merge);
    assert!(failure.stash && failure.retry.is_some(), "{failure:?}");

    act(&mut tab, stash_action(stash::Action::OpenAfter(source)));
    let retry = dialog_retry(&tab).expect("offered");
    assert_eq!(retry.button(), "Stash and merge");
    assert_eq!(retry.label(), "Merge of “topic” into “main”");
    assert_eq!(retry.command(), "merge.start");
    assert_eq!(retry.params()["rev"], json!("refs/heads/topic"));
    assert_eq!(retry.params().get("confirm"), None);

    act(&mut tab, stash_action(stash::Action::SubmitAndRetry));
    assert!(repo(&tab).failure().is_none(), "{:?}", repo(&tab).failure());
    assert_eq!(stashes(&tab), 1);
    assert_ne!(repo(&tab).head, before, "the merge moved main");
    assert!(
        tab.notice.starts_with("Changes stashed in stash@{0}. "),
        "{}",
        tab.notice
    );
    act(&mut tab, undo());
    assert_eq!(repo(&tab).head, before);
    act(&mut tab, undo());
    assert_eq!(stashes(&tab), 0);
}

#[test]
fn a_retry_refused_again_shows_its_dialog_and_the_stash_stays() {
    // `notes` has `note.txt`, which is untracked here: a stash without "Include untracked
    // files" does not set it aside, and the checkout is refused again.
    let mut backend = MemoryBackend::sample();
    let commit = add_commit(&mut backend, 30, 12, Some("note.txt"));
    backend.insert_commit(commit);
    backend.set_branch("notes", oid(30));
    let mut tab = tab_with(backend);

    act(&mut tab, checkout("notes"));
    assert_eq!(repo(&tab).failure().map(|f| f.0), Some(Source::Checkout));
    act(
        &mut tab,
        stash_action(stash::Action::OpenAfter(Source::Checkout)),
    );
    act(&mut tab, stash_action(stash::Action::SubmitAndRetry));

    // The stash is made, the checkout was sent once and refused: the usual dialog explains.
    assert_eq!(stashes(&tab), 1);
    assert_eq!(branch_name(&tab), Some("main"));
    let (source, failure) = repo(&tab).failure().expect("refused again");
    assert_eq!(source, Source::Checkout);
    assert!(failure.explanation.contains("note.txt"), "{failure:?}");
    assert!(!tab.executor.is_busy());
    assert_eq!(
        *tab.notice,
        *"Did not complete: Checking out “notes”. The changes stay in stash@{0}: bring them back with \
          Apply or Pop from the Stashes section."
    );

    // From that dialog the flow starts over: with the untracked file in the stash it goes.
    act(
        &mut tab,
        stash_action(stash::Action::OpenAfter(Source::Checkout)),
    );
    assert!(dialog_retry(&tab).is_some());
    let fields = stash::PushFields {
        include_untracked: true,
        ..Default::default()
    };
    act(&mut tab, stash_action(stash::Action::Fields(fields)));
    act(&mut tab, stash_action(stash::Action::SubmitAndRetry));
    assert_eq!(stashes(&tab), 2);
    assert_eq!(branch_name(&tab), Some("notes"));
    assert!(repo(&tab).failure().is_none());
}
