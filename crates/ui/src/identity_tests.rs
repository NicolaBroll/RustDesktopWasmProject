//! The "Git" section of the settings on a tab with the `MemoryBackend`: what is read and when,
//! what "Save" sends, a write that fails, and the offer after a command stopped by a missing
//! identity.

use super::*;
use crate::executor::Executor;
use crate::failure::Source;
use crate::{ErrorKind, branch};
use project_engine::{
    Capabilities, MemoryBackend, MemoryOpener, Platform, Registry, RepoOpener, Session,
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

const REPO: &str = "r";

/// The sample repo with its own name and email.
fn sample() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    repo.set_identity("Ada", "ada@example.com");
    repo
}

/// The user "A. L." with `repo` (if any) at [`REPO`].
fn opener(repo: Option<MemoryBackend>) -> Arc<MemoryOpener> {
    let mut opener = MemoryOpener::new();
    if let Some(repo) = repo {
        opener.insert(REPO, repo);
    }
    let set = |key, value| opener.set_global_identity(key, Some(value)).unwrap();
    set(IdentityKey::Name, "A. L.");
    set(IdentityKey::Email, "al@example.com");
    Arc::new(opener)
}

/// A tab on `opener`, with the repo at [`REPO`] open if `open`.
fn tab(opener: &Arc<MemoryOpener>, open: bool) -> Tab {
    let shared: Arc<dyn RepoOpener> = opener.clone();
    let executor = Executor::new(
        Session::new(shared),
        Registry::builtin(),
        Arc::new(Inline),
        Arc::new(|| {}),
    );
    let mut tab = Tab::new(7, executor);
    if open {
        tab.open(REPO.to_owned());
        settle(&mut tab);
        assert!(tab.repo.is_some(), "{}", tab.error);
    }
    tab
}

fn settle(tab: &mut Tab) {
    while tab.executor.is_busy() {
        tab.receive();
    }
}

fn context(tab: &Tab) -> Context {
    Context {
        tab: tab.id,
        repo: tab.repo_path().map(str::to_owned),
    }
}

/// What the app does in a frame with the section shown: the read if it is wanted, then the
/// answers. Returns whether a read was sent and what the answers were.
fn frame(form: &mut IdentityBox, tab: &mut Tab) -> (bool, Vec<Received>) {
    let context = context(tab);
    let read = form.wants(&context);
    if read {
        let ticket = tab.read_identity();
        form.reading(context, ticket);
    }
    settle(tab);
    let answers = std::mem::take(&mut tab.identity_done)
        .into_iter()
        .map(|done| form.receive(tab.id, done))
        .collect();
    (read, answers)
}

/// "Save": the parameters sent and the answer.
fn save(form: &mut IdentityBox, tab: &mut Tab) -> (Value, Received) {
    let params = form.save().expect("something to save");
    let ticket = tab.save_identity(params.clone());
    form.sending(tab.id, ticket);
    assert!(form.is_saving() && !form.can_save());
    let (read, mut answers) = frame(form, tab);
    assert!(!read, "no read beside the save");
    assert_eq!(answers.len(), 1);
    (params, answers.remove(0))
}

/// The section just opened on `tab`, read.
fn shown(tab: &mut Tab) -> IdentityBox {
    let mut form = IdentityBox::default();
    form.show();
    assert_eq!(frame(&mut form, tab), (true, vec![Received::Read]));
    form
}

fn texts(form: &IdentityBox) -> (&str, &str) {
    (form.text(IdentityKey::Name), form.text(IdentityKey::Email))
}

#[test]
fn the_section_reads_when_it_is_shown_and_not_again() {
    let opener = opener(Some(sample()));
    opener.set_identity_env("GIT_AUTHOR_NAME", "Robot");
    let mut tab = tab(&opener, true);
    let mut form = IdentityBox::default();
    assert!(form.identity().is_none() && !form.can_save());
    form.show();
    assert_eq!(frame(&mut form, &mut tab), (true, vec![Received::Read]));
    // The frames that follow read nothing.
    for _ in 0..3 {
        assert_eq!(frame(&mut form, &mut tab), (false, vec![]));
    }

    // The repo has its own name and email: the form starts on them.
    let identity = form.identity().expect("read");
    assert_eq!(identity.name.value.as_deref(), Some("Ada"));
    assert_eq!(identity.name.scope, Some(ConfigScope::Local));
    assert_eq!(form.repo(), Some(REPO));
    assert_eq!(form.scope(), IdentityScope::Local);
    assert_eq!(texts(&form), ("Ada", "ada@example.com"));
    assert!(!form.can_save(), "nothing changed yet");
    // The values for all repositories are hidden here, and the environment wins over both.
    assert_eq!(hidden(identity, IdentityKey::Name), Some("A. L."));
    assert_eq!(hidden(identity, IdentityKey::Email), Some("al@example.com"));
    assert_eq!(environment(identity), (vec!["GIT_AUTHOR_NAME"], false));

    // Shown again (another section and back): one more read, the values of now.
    opener
        .set_global_identity(IdentityKey::Name, Some("Lovelace"))
        .unwrap();
    form.show();
    assert_eq!(frame(&mut form, &mut tab), (true, vec![Received::Read]));
    assert_eq!(
        hidden(form.identity().unwrap(), IdentityKey::Name),
        Some("Lovelace")
    );
}

#[test]
fn save_sends_the_scope_and_only_the_fields_that_changed() {
    let opener = opener(Some(sample()));
    let mut tab = tab(&opener, true);
    let mut form = shown(&mut tab);

    // "For all repositories": the untouched fields show the values of that scope.
    form.apply(Action::Scope(IdentityScope::Global));
    assert_eq!(texts(&form), ("A. L.", "al@example.com"));
    form.apply(Action::Email("lovelace@example.com".to_owned()));
    let (params, answer) = save(&mut form, &mut tab);
    assert_eq!(
        params,
        json!({"scope": "global", "email": "lovelace@example.com"})
    );
    let notice = "Name and email saved for all repositories. Before: name “A. L.”, email \
                  “al@example.com”.";
    assert_eq!(answer, Received::Saved(notice.to_owned()));
    assert_eq!(form.outcome(), Some(&Ok(notice.to_owned())));
    let global = opener.identity().unwrap().global;
    assert_eq!(global.name.as_deref(), Some("A. L."));
    assert_eq!(global.email.as_deref(), Some("lovelace@example.com"));
    assert!(!form.can_save(), "saved: nothing left to write");
    assert_eq!(form.save(), None);

    // An emptied field removes the key from that scope.
    form.apply(Action::Name(String::new()));
    let (params, _) = save(&mut form, &mut tab);
    assert_eq!(params, json!({"scope": "global", "name": ""}));
    assert_eq!(opener.identity().unwrap().global.name, None);

    // "Only for this repository" writes the repo's own configuration, and nothing else.
    form.apply(Action::Scope(IdentityScope::Local));
    assert_eq!(texts(&form), ("Ada", "ada@example.com"));
    form.apply(Action::Name("Grace".to_owned()));
    let (params, answer) = save(&mut form, &mut tab);
    assert_eq!(params, json!({"scope": "local", "name": "Grace"}));
    let notice = "Name and email saved for this repository. Before: name “Ada”, email \
                  “ada@example.com”.";
    assert_eq!(answer, Received::Saved(notice.to_owned()));
    let identity = form.identity().unwrap();
    assert_eq!(identity.local.name.as_deref(), Some("Grace"));
    assert_eq!(identity.name.value.as_deref(), Some("Grace"));
    assert_eq!(
        opener.identity().unwrap().global.name,
        None,
        "global untouched"
    );
}

#[test]
fn a_failed_write_shows_the_error_and_keeps_what_was_typed() {
    let opener = opener(Some(sample()));
    opener.fail_identity_writes(IdentityKey::Email, "error: could not lock config file");
    let mut tab = tab(&opener, true);
    let mut form = shown(&mut tab);
    form.apply(Action::Scope(IdentityScope::Global));
    form.apply(Action::Name("Grace".to_owned()));
    form.apply(Action::Email("grace@example.com".to_owned()));
    let (params, answer) = save(&mut form, &mut tab);
    assert_eq!(
        params,
        json!({"scope": "global", "name": "Grace", "email": "grace@example.com"})
    );
    let Received::Failed(message) = answer else {
        panic!("{answer:?}");
    };
    assert!(
        message.starts_with("Name and email not saved: "),
        "{message}"
    );
    assert!(message.contains("could not lock config file"), "{message}");
    let Some(Err(error)) = form.outcome() else {
        panic!("{:?}", form.outcome());
    };
    assert!(error.contains("could not lock config file"), "{error}");
    assert_eq!(texts(&form), ("Grace", "grace@example.com"));

    // The name was written before the email failed: the values are read again, the form
    // keeps what was typed and only the email is left to save.
    assert_eq!(frame(&mut form, &mut tab), (true, vec![Received::Read]));
    assert_eq!(texts(&form), ("Grace", "grace@example.com"));
    assert!(
        form.outcome().is_some_and(|o| o.is_err()),
        "the error stays"
    );
    assert_eq!(
        form.save(),
        Some(json!({"scope": "global", "email": "grace@example.com"}))
    );

    // A value the engine refuses: nothing written, the text stays to be fixed.
    form.apply(Action::Email("<grace@example.com>".to_owned()));
    let (_, answer) = save(&mut form, &mut tab);
    assert!(matches!(answer, Received::Failed(_)), "{answer:?}");
    assert_eq!(form.text(IdentityKey::Email), "<grace@example.com>");
    assert_eq!(
        opener.identity().unwrap().global.email.as_deref(),
        Some("al@example.com")
    );
}

#[test]
fn without_a_repo_only_the_values_for_all_repositories() {
    let opener = opener(None);
    let mut tab = tab(&opener, false);
    let mut form = shown(&mut tab);
    assert_eq!(form.repo(), None);
    assert_eq!(form.scope(), IdentityScope::Global);
    assert_eq!(texts(&form), ("A. L.", "al@example.com"));
    // There is no "this repository" to choose.
    form.apply(Action::Scope(IdentityScope::Local));
    assert_eq!(form.scope(), IdentityScope::Global);
    form.apply(Action::Name("Grace".to_owned()));
    let (params, answer) = save(&mut form, &mut tab);
    assert_eq!(params, json!({"scope": "global", "name": "Grace"}));
    assert!(matches!(answer, Received::Saved(_)), "{answer:?}");
    assert_eq!(
        opener.identity().unwrap().global.name.as_deref(),
        Some("Grace")
    );
}

#[test]
fn nothing_set_anywhere_starts_on_all_repositories_with_empty_fields() {
    let mut repo = MemoryBackend::sample();
    repo.clear_identity();
    let mut plain = MemoryOpener::new();
    plain.insert(REPO, repo);
    plain.set_identity_env("EMAIL", "env@example.com");
    let opener = Arc::new(plain);
    let mut tab = tab(&opener, true);
    let mut form = shown(&mut tab);
    assert_eq!(form.scope(), IdentityScope::Global);
    assert_eq!(texts(&form), ("", ""));
    assert!(!form.can_save());
    let identity = form.identity().unwrap();
    assert_eq!((&identity.name.value, identity.name.scope), (&None, None));
    // No email configured: git would take the one of the environment.
    assert_eq!(environment(identity), (vec![], true));
    // With "Only for this repository" an empty field falls back to all repositories.
    form.apply(Action::Name("Grace".to_owned()));
    save(&mut form, &mut tab);
    form.apply(Action::Scope(IdentityScope::Local));
    assert_eq!(form.text(IdentityKey::Name), "");
    assert_eq!(form.inherited(IdentityKey::Name), Some("Grace"));
    assert_eq!(form.inherited(IdentityKey::Email), None);
}

#[test]
fn a_field_that_was_edited_keeps_its_text_across_scopes_and_reads() {
    let opener = opener(Some(sample()));
    let mut tab = tab(&opener, true);
    let mut form = shown(&mut tab);
    form.apply(Action::Name("Typed".to_owned()));
    // The email follows the scope, the edited name stays.
    form.apply(Action::Scope(IdentityScope::Global));
    assert_eq!(texts(&form), ("Typed", "al@example.com"));
    opener
        .set_global_identity(IdentityKey::Email, Some("new@example.com"))
        .unwrap();
    form.show();
    frame(&mut form, &mut tab);
    assert_eq!(texts(&form), ("Typed", "new@example.com"));
    assert_eq!(
        form.save(),
        Some(json!({"scope": "global", "name": "Typed"}))
    );
}

#[test]
fn closing_drops_the_form_and_a_save_in_progress_still_answers() {
    let opener = opener(Some(sample()));
    let mut tab = tab(&opener, true);
    let mut form = shown(&mut tab);
    form.apply(Action::Name("Grace".to_owned()));
    let params = form.save().unwrap();
    let ticket = tab.save_identity(params);
    form.sending(tab.id, ticket);
    form.close();
    assert!(form.identity().is_none() && form.text(IdentityKey::Name).is_empty());
    assert!(!form.wants(&context(&tab)), "not beside the save");
    settle(&mut tab);
    let done = tab.identity_done.remove(0);
    assert!(matches!(form.receive(tab.id, done), Received::Saved(_)));
    assert!(form.outcome().is_none(), "the form that sent it is gone");
    // Opened again: a fresh read, the saved value.
    form.show();
    frame(&mut form, &mut tab);
    assert_eq!(form.text(IdentityKey::Name), "Grace");
}

#[test]
fn another_repo_in_the_tab_starts_the_form_again() {
    let opener = opener(Some(sample()));
    let mut tab = tab(&opener, false);
    let mut form = shown(&mut tab);
    form.apply(Action::Name("Typed".to_owned()));
    // The repo opens under the window: its values, not what was typed for no repo.
    tab.open(REPO.to_owned());
    settle(&mut tab);
    assert_eq!(frame(&mut form, &mut tab), (true, vec![Received::Read]));
    assert_eq!(form.repo(), Some(REPO));
    assert_eq!(texts(&form), ("Ada", "ada@example.com"));
}

#[test]
fn the_focus_is_asked_once_for_the_first_empty_field() {
    let opener = opener(None);
    opener
        .set_global_identity(IdentityKey::Email, None)
        .unwrap();
    let mut tab = tab(&opener, false);
    let mut form = IdentityBox::default();
    assert_eq!(form.focus(), None);
    form.show_focused();
    frame(&mut form, &mut tab);
    assert_eq!(form.focus(), Some(IdentityKey::Email));
    form.apply(Action::Focused);
    assert_eq!(form.focus(), None);
}

/// A failed answer of `command` as the executor delivers it.
fn failed(command: &str, stderr: &str) -> Done {
    Done {
        ticket: Ticket::for_test(900),
        command: command.to_owned(),
        result: Err(format!("`git {command}` failed: {stderr}")),
        error_kind: Some(ErrorKind::Git),
        error_detail: Some(stderr.to_owned()),
        confirm_expected: None,
        confirm_bytes: None,
    }
}

const UNKNOWN: &str = "Committer identity unknown\n\n*** Please tell me who you are.";

#[test]
fn a_command_stopped_by_a_missing_identity_offers_the_settings_in_a_dialog() {
    let opener = opener(Some(sample()));
    let mut tab = tab(&opener, true);
    let generation = tab.generation;
    // An annotated tag: its error stays in the tag dialog, which has no such button.
    let request = Request::Branch(generation, branch::Op::CreateTag);
    tab.receive_one(request, failed("tag", UNKNOWN));
    let repo = tab.repo.as_mut().unwrap();
    repo.queue_failures();
    let (source, failure) = repo.failure().expect("offered");
    assert_eq!(source, Source::Identity);
    assert_eq!(failure.title, "Git needs your name and email");
    assert_eq!(failure.git.as_deref(), Some(UNKNOWN));
    assert!(failure.identity && repo.modal_open());
    tab.apply(Source::Identity.dismiss());
    assert!(tab.repo.as_ref().unwrap().failure().is_none());

    // Any other failure of the same command offers nothing.
    tab.receive_one(request, failed("tag", "fatal: tag 'v1' already exists"));
    assert!(tab.repo.as_ref().unwrap().failure().is_none());

    // Offered again, then the identity is saved: the offer goes away.
    tab.receive_one(request, failed("tag", UNKNOWN));
    assert!(tab.repo.as_ref().unwrap().offers_identity());
    tab.identity_saved();
    assert!(!tab.repo.as_ref().unwrap().offers_identity());
}

#[test]
fn an_answer_for_a_repo_no_longer_open_offers_nothing() {
    let opener = opener(Some(sample()));
    let mut tab = tab(&opener, true);
    let old = tab.generation;
    tab.open(REPO.to_owned());
    settle(&mut tab);
    assert_ne!(tab.generation, old);
    tab.offer_identity(old, None);
    assert!(!tab.repo.as_ref().unwrap().offers_identity());
}
