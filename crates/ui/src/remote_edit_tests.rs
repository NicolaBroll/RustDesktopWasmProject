use super::*;
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use std::sync::Arc;

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

fn listed(registry: &Registry, session: &mut Session) -> RemotesBox {
    let mut b = RemotesBox::default();
    b.receive_list(registry.run(LIST, session, json!({})).map_err(|e| e.text()));
    b
}

fn run(r: &Registry, s: &mut Session, b: &mut RemotesBox, op: Op, params: Value) -> Received {
    let result = r.run(op.command(), s, params);
    let kind = result.as_ref().err().map(ErrorKind::of);
    b.receive(op, result.map_err(|e| e.text()), kind, None, None)
}

fn edit(b: &RemotesBox) -> &EditDialog {
    match b.dialog() {
        Some(Dialog::Edit(dialog)) => dialog,
        other => panic!("{other:?}"),
    }
}

fn type_fields(b: &mut RemotesBox, f: impl FnOnce(&mut EditFields)) {
    let mut fields = edit(b).fields.clone();
    f(&mut fields);
    b.set_edit_fields(fields);
}

#[test]
fn transports_are_recognised() {
    assert_eq!(transport("https://h/r.git"), Some(Transport::Https));
    assert_eq!(transport("HTTP://h/r"), Some(Transport::Https));
    assert_eq!(transport("ssh://git@h/r"), Some(Transport::Ssh));
    assert_eq!(transport("git@github.com:x/y.git"), Some(Transport::Ssh));
    assert_eq!(transport("/srv/r.git"), None);
    assert_eq!(transport("C:\\repos\\r"), None);
    assert_eq!(transport("file:///srv/r"), None);
    assert_eq!(transport("git://h/r"), None);
    assert_eq!(transport("./a:b"), None);
}

#[test]
fn edit_url_prefills_only_unmasked_urls() {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("fork", "https://anna:secret@example.com/fork.git");
    let (registry, mut session) = opened(repo);
    let mut b = listed(&registry, &mut session);

    assert!(b.open_edit("origin", EditKind::Url));
    assert!(
        !b.open_edit("origin", EditKind::Rename),
        "one dialog at a time"
    );
    let d = edit(&b);
    assert_eq!(d.fields.url, "https://example.com/project.git");
    assert_eq!(d.fields.push_url, "");
    assert_eq!(d.masked_note(), None);
    assert!(!d.can_submit(), "nothing changed yet");
    assert_eq!(d.transport_note(), None);
    b.close();

    assert!(b.open_edit("fork", EditKind::Url));
    let d = edit(&b);
    assert_eq!(d.fields.url, "", "a masked URL is never prefilled");
    assert!(d.url_hint().contains("***"), "{}", d.url_hint());
    assert!(!d.url_hint().contains("secret"));
    assert!(d.masked_note().is_some());
    assert!(!d.can_submit());
    b.close();
    assert!(!b.open_edit("nobody", EditKind::Url));
}

#[test]
fn edit_url_with_a_push_url_in_two_steps_and_undo() {
    let (registry, mut session) = opened(MemoryBackend::sample());
    let mut b = listed(&registry, &mut session);
    b.open_edit("origin", EditKind::Url);
    type_fields(&mut b, |f| {
        f.url = " git@example.com:team/project.git ".into();
        f.push_url = "ssh://git@example.com/push.git".into();
    });
    let d = edit(&b);
    assert!(d.transport_note().unwrap().contains("From HTTPS to SSH"));
    assert_eq!(d.credentials_warning(), None);
    let (op, params, label) = b.submit_edit().unwrap();
    assert_eq!(op, Op::SetUrl);
    assert_eq!(
        params,
        json!({ "name": "origin", "url": "git@example.com:team/project.git" })
    );
    assert_eq!(label, "Changing the URL of “origin”");
    assert_eq!(b.submit_edit(), None, "only once");
    b.close();
    assert!(b.modal_open(), "it does not close during the submit");

    let Received::Step {
        outcome,
        op,
        params,
        label,
    } = run(&registry, &mut session, &mut b, op, params)
    else {
        panic!()
    };
    assert!(
        outcome
            .notice
            .starts_with("URL of “origin” changed to git@example.com:team/project.git"),
        "{}",
        outcome.notice
    );
    assert!(outcome.history.unwrap().can_undo);
    assert_eq!(op, Op::SetUrl);
    assert_eq!(params["push"], true);
    assert_eq!(label, "Changing the push URL of “origin”");
    assert!(b.modal_open());

    let Received::Done(outcome) = run(&registry, &mut session, &mut b, op, params) else {
        panic!()
    };
    assert!(outcome.notice.contains("URL of “origin” changed to git@"));
    assert!(
        outcome
            .notice
            .contains("Push URL of “origin” changed to ssh://git@example.com/push.git"),
        "{}",
        outcome.notice
    );
    assert!(b.dialog().is_none());
    let origin = &b.list().unwrap()[0];
    assert_eq!(
        origin.url.as_deref(),
        Some("git@example.com:team/project.git")
    );
    assert_eq!(origin.push_urls, ["ssh://git@example.com/push.git"]);

    // Two entries in the journal: two undos give back the first URL.
    registry.run("edit.undo", &mut session, json!({})).unwrap();
    registry.run("edit.undo", &mut session, json!({})).unwrap();
    let b = listed(&registry, &mut session);
    let origin = &b.list().unwrap()[0];
    assert_eq!(
        origin.url.as_deref(),
        Some("https://example.com/project.git")
    );
    assert!(origin.push_urls.is_empty());
}

#[test]
fn edit_url_refused_stays_in_the_dialog() {
    let (registry, mut session) = opened(MemoryBackend::sample());
    let mut b = listed(&registry, &mut session);
    b.open_edit("origin", EditKind::Url);
    type_fields(&mut b, |f| f.url = "--upload-pack=x".into());
    let (op, params, _) = b.submit_edit().unwrap();
    let r = run(&registry, &mut session, &mut b, op, params);
    assert!(matches!(r, Received::Inline), "{r:?}");
    let d = edit(&b);
    assert!(d.error.is_some());
    assert!(!d.sending);
    assert!(d.can_submit(), "it can be corrected and sent again");
    // A URL with credentials: warning before sending.
    type_fields(&mut b, |f| {
        f.url = "https://anna:pw@example.com/r.git".into()
    });
    assert!(edit(&b).credentials_warning().is_some());
}

#[test]
fn rename_moves_branches_and_upstreams_and_undo() {
    let (registry, mut session) = opened(MemoryBackend::sample());
    let mut b = listed(&registry, &mut session);
    b.open_edit("origin", EditKind::Rename);
    assert_eq!(edit(&b).fields.new_name, "origin");
    assert!(!edit(&b).can_submit(), "same name");

    // An invalid name: the engine's message stays in the dialog.
    type_fields(&mut b, |f| f.new_name = "bad name".into());
    let (op, params, _) = b.submit_edit().unwrap();
    assert_eq!(op, Op::Rename);
    let r = run(&registry, &mut session, &mut b, op, params);
    assert!(matches!(r, Received::Inline), "{r:?}");
    assert!(edit(&b).error.is_some());

    type_fields(&mut b, |f| f.new_name = " upstream ".into());
    let (op, params, label) = b.submit_edit().unwrap();
    assert_eq!(params, json!({ "name": "origin", "new_name": "upstream" }));
    assert_eq!(label, "Renaming remote “origin” to “upstream”");
    let Received::Done(outcome) = run(&registry, &mut session, &mut b, op, params) else {
        panic!()
    };
    assert!(
        outcome.notice.starts_with(
            "Remote “origin” renamed to “upstream”; 2 remote-tracking branches moved; upstream of “"
        ),
        "{}",
        outcome.notice
    );
    assert!(outcome.notice.contains("“Undo” renames it back"));
    let refs = outcome.refs.unwrap();
    assert!(refs.iter().any(|r| r.name == "refs/remotes/upstream/main"));
    assert!(outcome.history.unwrap().can_undo);
    assert_eq!(b.names(), ["upstream"]);
    assert!(b.dialog().is_none());

    registry.run("edit.undo", &mut session, json!({})).unwrap();
    assert_eq!(listed(&registry, &mut session).names(), ["origin"]);
}
