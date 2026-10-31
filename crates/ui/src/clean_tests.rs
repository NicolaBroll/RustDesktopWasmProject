use super::*;
use crate::discard::{History, UndoOp};
use crate::stage::{Side, StageLists};
use project_domain::Conflict;
use project_engine::{EngineError, MemoryOpener, Registry, Session};
use std::sync::Arc;

fn item(path: &str, change: Change, conflict: Option<Conflict>) -> StageItem {
    StageItem {
        path: path.into(),
        change,
        original: None,
        conflict,
    }
}

#[test]
fn only_untracked_files_are_deletable() {
    let items = [
        item("zeta.rs", Change::Unmerged, Some(Conflict::BothModified)),
        item("README.md", Change::Modified, None),
        item("note.txt", Change::Untracked, None),
        item("build/out.log", Change::Untracked, None),
    ];
    let can: Vec<bool> = items.iter().map(deletable).collect();
    assert_eq!(can, [false, false, true, true]);
    assert_eq!(
        untracked_paths(&items).unwrap(),
        ["note.txt", "build/out.log"].map(RepoPath::from)
    );
    assert_eq!(untracked_paths(&items[..2]), None);
    assert_eq!(untracked_paths(&[]), None);
}

#[test]
fn sizes_are_readable() {
    assert_eq!(size_text(1), "1 byte");
    assert_eq!(size_text(512), "512 bytes");
    assert_eq!(size_text(1536), "1.5 KB");
    assert_eq!(size_text(3 * 1024 * 1024), "3.0 MB");
}

#[test]
fn confirm_is_sent_only_after_the_dialog() {
    let mut b = CleanBox::default();
    assert_eq!(b.start(Vec::new()), None);
    let check = b.start(vec!["note.txt".into()]).unwrap();
    assert_eq!(check, json!({ "paths": ["note.txt"] }));
    // One at a time, and no confirmation before the engine's response.
    assert_eq!(b.start(vec!["other".into()]), None);
    assert_eq!(b.confirmed(), None);
    assert!(b.confirm().is_none());

    let r = b.receive(
        Err("asks for confirmation".to_owned()),
        Some(ErrorKind::ConfirmRequired),
        Some(42),
    );
    assert!(matches!(r, Received::Confirm));
    let confirm = b.confirm().unwrap();
    assert_eq!(confirm.phase, Phase::Confirm { bytes: Some(42) });
    assert_eq!(confirm.paths, [RepoPath::from("note.txt")]);

    let send = b.confirmed().unwrap();
    assert_eq!(send, json!({ "paths": ["note.txt"], "confirm": true }));
    assert!(b.confirm().is_none());
    assert_eq!(b.current().unwrap().phase, Phase::Sending);
    assert_eq!(b.confirmed(), None);
}

#[test]
fn cancelling_the_dialog_sends_nothing() {
    let mut b = CleanBox::default();
    b.start(vec!["a".into(), "b".into()]).unwrap();
    b.receive(Err(String::new()), Some(ErrorKind::ConfirmRequired), None);
    b.cancel();
    assert!(b.current().is_none());
    assert_eq!(b.confirmed(), None);
}

#[test]
fn refusals_are_shown_and_close_the_deletion() {
    let mut b = CleanBox::default();
    b.start(vec!["README.md".into()]).unwrap();
    let r = b.receive(
        Err("not an untracked file".to_owned()),
        Some(ErrorKind::NotAllowed),
        None,
    );
    let Received::Failed { message, reload } = r else {
        panic!("{r:?}");
    };
    assert_eq!(
        message,
        "Deleting README.md not possible: not an untracked file"
    );
    assert!(!reload);
    assert!(b.current().is_none());

    // A confirmation requested again after `confirm: true` does not reopen the dialog forever.
    b.start(vec!["a".into(), "b".into()]).unwrap();
    b.receive(Err(String::new()), Some(ErrorKind::ConfirmRequired), None);
    b.confirmed().unwrap();
    let r = b.receive(Err("x".to_owned()), Some(ErrorKind::ConfirmRequired), None);
    assert!(matches!(r, Received::Failed { .. }), "{r:?}");
    assert!(b.current().is_none());
}

fn open() -> (Registry, Session) {
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(MemoryOpener::sample()));
    registry
        .run(
            "repo.open",
            &mut session,
            json!({ "path": MemoryOpener::SAMPLE_PATH }),
        )
        .unwrap();
    (registry, session)
}

fn lists(out: &Value) -> StageLists {
    StageLists::new(&serde_json::from_value(out["status"].clone()).unwrap())
}

/// The full path with the sample repo of the `MemoryBackend`, as the UI does it: check with
/// the size, confirmation, deletion, then undo from the button.
#[test]
fn deleting_with_the_sample_repo_then_undo() {
    let (registry, mut session) = open();
    let status = registry
        .run("repo.status", &mut session, json!({}))
        .unwrap();
    let before = StageLists::new(&serde_json::from_value(status).unwrap());
    let paths = untracked_paths(&before.unstaged).unwrap();
    assert!(paths.contains(&"note.txt".into()), "{paths:?}");

    // The check: nothing changes, confirmation is needed, with the total size.
    let mut b = CleanBox::default();
    let check = b.start(paths.clone()).unwrap();
    let err = registry.run(COMMAND, &mut session, check).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    let bytes = ErrorKind::bytes(&err);
    assert!(bytes.is_some_and(|b| b > 0), "{bytes:?}");
    let r = b.receive(Err(err.text()), Some(ErrorKind::of(&err)), bytes);
    assert!(matches!(r, Received::Confirm));
    assert_eq!(b.confirm().unwrap().paths, paths);

    // The confirmation.
    let send = b.confirmed().unwrap();
    let out = registry.run(COMMAND, &mut session, send).unwrap();
    assert!(!lists(&out).contains(Side::Unstaged, &"note.txt".into()));
    let Received::Done(result) = b.receive(Ok(out), None, None) else {
        panic!()
    };
    assert!(result.backup.starts_with("refs/project/backup/clean-"));
    assert_eq!(result.deleted, paths);
    assert_eq!(Some(result.bytes), bytes);
    let notice = notice(&result);
    assert!(
        notice.contains("deleted") && notice.contains("Undo"),
        "{notice}"
    );
    let history = History::from(result.journal);
    assert!(history.can_undo);

    // Undo: the file comes back.
    let out = registry
        .run(UndoOp::Undo.command(), &mut session, json!({}))
        .unwrap();
    assert!(lists(&out).contains(Side::Unstaged, &"note.txt".into()));
}

#[test]
fn tracked_files_are_refused_before_the_dialog() {
    let (registry, mut session) = open();
    let mut b = CleanBox::default();
    let check = b.start(vec!["src/lib.rs".into()]).unwrap();
    let err = registry.run(COMMAND, &mut session, check).unwrap_err();
    let r = b.receive(Err(err.text()), Some(ErrorKind::of(&err)), None);
    let Received::Failed { message, .. } = r else {
        panic!("{r:?}")
    };
    assert!(message.contains("not possible"), "{message}");
    assert!(b.confirm().is_none());
}
