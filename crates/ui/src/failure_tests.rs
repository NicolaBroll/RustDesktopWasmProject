//! The failure dialogs on a tab with the `MemoryBackend`: one at a time in the order they came,
//! modal (nothing starts under them), the checkout refused for local changes with "Stash…".

use super::*;
use crate::executor::Executor;
use crate::{ErrorKind, Tab, stash};
use project_engine::{
    Capabilities, FetchFailure, MemoryBackend, MemoryOpener, Platform, Registry, Session,
};
use std::sync::Arc;

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

/// The sample repo (local changes to `README.md`) whose remote rejects the credentials.
fn tab() -> Tab {
    let mut repo = MemoryBackend::sample();
    let git = "fatal: could not read Username for 'https://example.com'";
    repo.fail_fetch("origin", Some(FetchFailure::Auth(git.into())));
    tab_with(repo)
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

/// The area and the title of the dialog on screen.
fn shown(tab: &Tab) -> Option<(Source, &str)> {
    repo(tab).failure().map(|(s, f)| (s, f.title.as_str()))
}

fn fetch_now() -> Action {
    Action::Fetch(fetch::Action::Fetch(None))
}

fn checkout(name: &str) -> Action {
    Action::Branch(branch::Action::Checkout(name.to_owned()))
}

/// The fetch box of a tab whose fetch just failed: a failure that arrives while another dialog
/// is open (the tab starts nothing under a dialog).
fn failed_fetch() -> fetch::FetchBox {
    let mut other = tab();
    act(&mut other, fetch_now());
    std::mem::take(&mut repo_mut(&mut other).fetch)
}

/// A checkout refused for `README.md` answered straight to the branch box (see
/// [`failed_fetch`]).
fn refuse_checkout(tab: &mut Tab) {
    let branch = &mut repo_mut(tab).branch;
    branch.start_checkout("feature/graph".to_owned()).unwrap();
    branch.receive(
        branch::Op::Checkout,
        Err("refused".to_owned()),
        Some(ErrorKind::CheckoutConflict),
        Some("README.md".to_owned()),
        None,
    );
}

#[test]
fn a_failure_is_a_modal_dialog_and_nothing_starts_under_it() {
    let mut tab = tab();
    assert!(!repo(&tab).modal_open());
    act(&mut tab, fetch_now());
    assert_eq!(shown(&tab), Some((Source::Fetch, "Authentication failed")));
    let view = repo(&tab);
    assert!(view.modal_open(), "counted among the modal dialogs");
    assert!(!view.dialog_open());
    assert!(view.auto_fetch_blocked());

    // No write starts under it: neither another fetch nor a checkout.
    tab.apply(fetch_now());
    tab.apply(checkout("feature/graph"));
    assert!(!tab.executor.is_busy());
    assert!(repo(&tab).branch.checkout.is_none());

    act(&mut tab, Source::Fetch.dismiss());
    assert_eq!(shown(&tab), None);
    assert!(!repo(&tab).modal_open());
}

#[test]
fn two_failures_are_shown_one_after_the_other() {
    // The refused checkout is on screen when the fetch fails: it stays, although the fetch
    // comes first among failures that arrive together.
    let mut tab = tab();
    refuse_checkout(&mut tab);
    repo_mut(&mut tab).queue_failures();
    assert_eq!(
        shown(&tab),
        Some((Source::Checkout, "Cannot check out “feature/graph”"))
    );
    repo_mut(&mut tab).fetch = failed_fetch();
    assert_eq!(shown(&tab).map(|s| s.0), Some(Source::Checkout));
    repo_mut(&mut tab).queue_failures();
    assert_eq!(shown(&tab).map(|s| s.0), Some(Source::Checkout));

    // Closing the first shows the next; closing that one leaves no dialog.
    act(&mut tab, Source::Checkout.dismiss());
    assert_eq!(shown(&tab), Some((Source::Fetch, "Authentication failed")));
    assert!(repo(&tab).modal_open());
    act(&mut tab, Source::Fetch.dismiss());
    assert_eq!(shown(&tab), None);
    assert!(!repo(&tab).modal_open());

    // Arrived together (before any frame): the fixed order, still one at a time.
    refuse_checkout(&mut tab);
    repo_mut(&mut tab).fetch = failed_fetch();
    assert_eq!(shown(&tab).map(|s| s.0), Some(Source::Fetch));
    act(&mut tab, Source::Fetch.dismiss());
    assert_eq!(shown(&tab).map(|s| s.0), Some(Source::Checkout));
    act(&mut tab, Source::Checkout.dismiss());
    assert_eq!(shown(&tab), None);
}

#[test]
fn a_refused_checkout_offers_the_stash() {
    let mut tab = tab();
    act(&mut tab, checkout("feature/graph"));
    let (source, failure) = repo(&tab).failure().expect("the refusal");
    assert_eq!(source, Source::Checkout);
    assert_eq!(failure.title, "Cannot check out “feature/graph”");
    assert!(
        failure
            .explanation
            .starts_with("You have uncommitted changes in "),
        "{failure:?}"
    );
    assert!(failure.explanation.contains("README.md"), "{failure:?}");
    assert!(failure.stash);
    assert!(repo(&tab).branch.dialog().is_none(), "no branch dialog");
    assert!(tab.error.is_empty(), "{:?}", tab.error);

    // "Stash…": the failure closes and the stash dialog opens.
    act(&mut tab, Source::Checkout.dismiss());
    act(&mut tab, Action::Stash(stash::Action::Open));
    assert!(repo(&tab).failure().is_none());
    assert!(repo(&tab).stash.dialog().is_some());
}

#[test]
fn stash_opens_from_a_failure_with_another_one_waiting() {
    let mut tab = tab();
    act(&mut tab, checkout("feature/graph"));
    repo_mut(&mut tab).fetch = failed_fetch();
    repo_mut(&mut tab).queue_failures();
    assert_eq!(shown(&tab).map(|s| s.0), Some(Source::Checkout));

    // "Stash…" in the dialog on screen: the failure still waiting does not hold the stash
    // dialog back, and is shown above it.
    act(&mut tab, Source::Checkout.dismiss());
    act(&mut tab, Action::Stash(stash::Action::Open));
    assert!(repo(&tab).stash.dialog().is_some());
    assert_eq!(shown(&tab).map(|s| s.0), Some(Source::Fetch));
}
