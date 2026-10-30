//! The clone and new-repository dialog on the `MemoryOpener`: fields and parameters, the folder
//! from "Browse…", the answers (opened, refused, authentication, unreachable, cancelled).

use super::*;
use project_engine::{
    CancelToken, EngineError, JobContext, MemoryBackend, MemoryOpener, Registry, Session,
};
use serde_json::json;
use std::sync::Arc;

fn session() -> (Registry, Session) {
    let mut opener = MemoryOpener::new();
    opener.insert("remote", MemoryBackend::sample());
    opener.occupy("full");
    opener.deny("private", "fatal: Authentication failed for 'private'");
    (Registry::builtin(), Session::new(Arc::new(opener)))
}

fn split(
    result: Result<Value, EngineError>,
) -> (Result<Value, String>, Option<ErrorKind>, Option<String>) {
    let (kind, detail) = match &result {
        Err(e) => (Some(ErrorKind::of(e)), ErrorKind::detail(e)),
        Ok(_) => (None, None),
    };
    (result.map_err(|e| e.text()), kind, detail)
}

/// A clone dialog with these fields.
fn dialog(kind: Kind, url: &str, path: &str) -> CloneBox {
    let mut b = CloneBox::default();
    b.open(kind);
    b.set_fields(Fields {
        url: url.into(),
        path: path.into(),
        ..Fields::default()
    });
    b
}

/// Sends the dialog like the app (ticket 1) and passes the answer back.
fn run(b: &mut CloneBox, r: &Registry, s: &mut Session, job: JobContext) -> Received {
    let (command, params) = b.submit().expect("can submit");
    let ticket = Ticket::for_test(1);
    b.started(ticket);
    assert!(!b.dialog().unwrap().can_submit(), "not twice");
    let (result, kind, detail) = split(r.run_with(command, s, params, job));
    b.receive(ticket, result, kind, detail)
}

#[test]
fn params_need_url_and_path_and_carry_branch_and_depth() {
    let mut b = dialog(Kind::Clone, " ", "dest");
    assert!(b.submit().is_none(), "no URL");
    b.set_fields(Fields {
        url: " remote ".into(),
        path: "dest".into(),
        branch: "main".into(),
        depth: "0".into(),
    });
    assert!(b.dialog().unwrap().problem().is_some());
    assert!(b.submit().is_none(), "depth 0");
    b.set_fields(Fields {
        depth: "3".into(),
        ..b.dialog().unwrap().fields.clone()
    });
    let (command, params) = b.submit().unwrap();
    assert_eq!(command, "repo.clone");
    assert_eq!(
        params,
        json!({"url": "remote", "path": "dest", "branch": "main", "depth": 3})
    );
    // Init: no URL needed, URL and depth never sent.
    let mut b = dialog(Kind::Init, "ignored", "new");
    b.set_fields(Fields {
        path: "new".into(),
        depth: "x".into(),
        ..Fields::default()
    });
    assert_eq!(b.submit().unwrap(), ("repo.init", json!({"path": "new"})));
}

#[test]
fn browse_adds_the_name_from_the_url_for_a_clone() {
    assert_eq!(repo_name("https://host/a/b.git"), Some("b"));
    assert_eq!(repo_name("git@host:x/y/"), Some("y"));
    assert_eq!(repo_name("target/demo-repo"), Some("demo-repo"));
    assert_eq!(repo_name("https://host/.."), None);
    let mut b = dialog(Kind::Clone, "https://host/a/b.git", "");
    b.set_folder("/work");
    let expected = Path::new("/work").join("b").to_string_lossy().into_owned();
    assert_eq!(b.dialog().unwrap().fields.path, expected);
    // Without a URL, and for a new repository: the folder itself.
    let mut b = dialog(Kind::Clone, "", "");
    b.set_folder("/work");
    assert_eq!(b.dialog().unwrap().fields.path, "/work");
    let mut b = dialog(Kind::Init, "", "");
    b.set_folder("/work/new");
    assert_eq!(b.dialog().unwrap().fields.path, "/work/new");
}

#[test]
fn a_clone_opens_the_repo_and_closes_the_dialog() {
    let (r, mut s) = session();
    let mut b = dialog(Kind::Clone, "remote", "dest");
    let Received::Opened { value, notice } = run(&mut b, &r, &mut s, JobContext::new()) else {
        panic!("not opened");
    };
    assert_eq!(value["path"], "dest");
    assert_eq!(notice, "");
    assert!(b.dialog().is_none());
    assert_eq!(s.path(), Some("dest"));
}

#[test]
fn init_opens_the_new_repo_on_the_chosen_branch() {
    let (r, mut s) = session();
    let mut b = dialog(Kind::Init, "", "new");
    b.set_fields(Fields {
        path: "new".into(),
        branch: "trunk".into(),
        ..Fields::default()
    });
    let Received::Opened { value, .. } = run(&mut b, &r, &mut s, JobContext::new()) else {
        panic!("not opened");
    };
    assert_eq!(value["head"]["Branch"]["name"], "trunk", "{value}");
    assert!(b.dialog().is_none());
}

#[test]
fn refusals_stay_in_the_dialog_with_the_reason() {
    let (r, mut s) = session();
    // A folder that is not empty.
    let mut b = dialog(Kind::Clone, "remote", "full");
    assert!(matches!(
        run(&mut b, &r, &mut s, JobContext::new()),
        Received::Failed
    ));
    let d = b.dialog().unwrap();
    let failure = d.failure.as_ref().unwrap();
    assert_eq!(failure.title, tr!("clone-not-possible-title"));
    assert!(failure.explanation.contains("full"), "{failure:?}");
    assert!(d.running.is_none() && d.can_submit(), "can retry");
    assert_eq!(s.path(), None);
    // A branch the remote does not have.
    let mut b = dialog(Kind::Clone, "remote", "dest");
    b.set_fields(Fields {
        branch: "nope".into(),
        ..b.dialog().unwrap().fields.clone()
    });
    assert!(matches!(
        run(&mut b, &r, &mut s, JobContext::new()),
        Received::Failed
    ));
    assert!(b.dialog().unwrap().failure.is_some());
}

#[test]
fn authentication_and_unreachable_remotes_are_explained() {
    let (r, mut s) = session();
    let mut b = dialog(Kind::Clone, "private", "dest");
    run(&mut b, &r, &mut s, JobContext::new());
    let failure = b.dialog().unwrap().failure.clone().unwrap();
    assert_eq!(failure.title, tr!("fetch-auth-title"));
    assert!(failure.explanation.contains("private"));
    assert!(failure.git.unwrap().contains("Authentication failed"));
    let mut b = dialog(Kind::Clone, "nowhere", "dest");
    run(&mut b, &r, &mut s, JobContext::new());
    let failure = b.dialog().unwrap().failure.clone().unwrap();
    assert_eq!(failure.title, tr!("fetch-unreachable-title"));
}

#[test]
fn cancel_is_sent_once_and_the_dialog_stays_without_errors() {
    let (r, mut s) = session();
    let mut b = dialog(Kind::Clone, "remote", "dest");
    assert_eq!(b.cancel(), None, "nothing running");
    let (command, params) = b.submit().unwrap();
    let ticket = Ticket::for_test(7);
    b.started(ticket);
    assert_eq!(b.cancel(), Some(ticket));
    assert_eq!(b.cancel(), None, "only once");
    // Not closed while it runs.
    b.close();
    assert!(b.dialog().is_some());
    let token = CancelToken::new();
    token.cancel();
    let job = JobContext::new().with_cancel(token);
    let (result, kind, detail) = split(r.run_with(command, &mut s, params, job));
    assert_eq!(kind, Some(ErrorKind::Cancelled));
    // An answer for another ticket is ignored.
    assert!(matches!(
        b.receive(Ticket::for_test(8), Ok(json!({})), None, None),
        Received::Unexpected
    ));
    assert!(matches!(
        b.receive(ticket, result, kind, detail),
        Received::Failed
    ));
    let d = b.dialog().unwrap();
    assert!(d.cancelled && d.failure.is_none() && d.running.is_none());
    b.close();
    assert!(b.dialog().is_none());
}

#[test]
fn fields_are_read_only_while_running_and_kinds_switch_only_when_idle() {
    let mut b = dialog(Kind::Clone, "remote", "dest");
    b.started(Ticket::for_test(1));
    b.set_fields(Fields::default());
    b.set_folder("/elsewhere");
    b.open(Kind::Init);
    let d = b.dialog().unwrap();
    assert_eq!((d.kind, d.fields.path.as_str()), (Kind::Clone, "dest"));
    let mut b = dialog(Kind::Clone, "remote", "dest");
    b.open(Kind::Clone);
    assert_eq!(b.dialog().unwrap().fields.url, "remote", "same kind: kept");
    b.open(Kind::Init);
    assert_eq!(b.dialog().unwrap().kind, Kind::Init);
}
