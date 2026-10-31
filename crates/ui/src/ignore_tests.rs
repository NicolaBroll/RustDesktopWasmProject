use super::*;
use crate::discard::{History, UndoOp};
use crate::stage::{Side, StageLists};
use project_domain::Conflict;
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use std::sync::Arc;

fn item(path: &str, change: Change, conflict: Option<Conflict>) -> StageItem {
    StageItem {
        path: path.into(),
        change,
        original: None,
        conflict,
    }
}

fn pairs(path: &str) -> Vec<(String, String)> {
    choices(&path.into())
        .into_iter()
        .map(|c| (c.label, c.pattern.to_string()))
        .collect()
}

#[test]
fn only_untracked_files_are_ignorable() {
    let items = [
        item("zeta.rs", Change::Unmerged, Some(Conflict::BothModified)),
        item("README.md", Change::Modified, None),
        item("note.txt", Change::Untracked, None),
    ];
    let can: Vec<bool> = items.iter().map(ignorable).collect();
    assert_eq!(can, [false, false, true]);
}

#[test]
fn choices_are_the_file_the_extension_and_each_folder() {
    let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    assert_eq!(
        pairs("build/out/app.log"),
        [
            s("This file", "/build/out/app.log"),
            s("All *.log files", "*.log"),
            s("Folder build/out/", "/build/out/"),
            s("Folder build/", "/build/"),
        ]
    );
    // No extension: no "All ... files"; special bytes escaped.
    assert_eq!(pairs("Makefile"), [s("This file", "/Makefile")]);
    assert_eq!(pairs(".env"), [s("This file", "/.env")]);
    assert_eq!(
        pairs("#notes/a b"),
        [
            s("This file", "/\\#notes/a b"),
            s("Folder #notes/", "/\\#notes/")
        ]
    );
}

#[test]
fn params_and_one_at_a_time() {
    let mut b = IgnoreBox::default();
    let send = b.start("*.log".into(), IgnoreFile::Exclude).unwrap();
    assert_eq!(send, json!({ "patterns": ["*.log"], "file": "exclude" }));
    assert_eq!(b.start("/a".into(), IgnoreFile::Gitignore), None);
    assert_eq!(
        params(&"/a".into(), IgnoreFile::Gitignore),
        json!({ "patterns": ["/a"], "file": "gitignore" })
    );
}

#[test]
fn refusals_are_shown_and_close_the_ignore() {
    let mut b = IgnoreBox::default();
    b.start("/a".into(), IgnoreFile::Gitignore).unwrap();
    let r = b.receive(
        Err("`.gitignore` is a link".to_owned()),
        Some(ErrorKind::NotAllowed),
    );
    let Received::Failed { message, reload } = r else {
        panic!("{r:?}");
    };
    assert_eq!(
        message,
        "Ignoring “/a” in .gitignore not possible: `.gitignore` is a link"
    );
    assert!(!reload);
    assert!(b.current().is_none());
    // A response nobody waits for: the status is reread.
    let r = b.receive(Ok(json!({})), None);
    assert!(matches!(r, Received::Failed { reload: true, .. }), "{r:?}");
}

fn open() -> (Registry, Session) {
    let mut repo = MemoryBackend::sample();
    repo.add_untracked("build/out.log", "log\n");
    repo.add_untracked("debug.log", "x");
    let mut opener = MemoryOpener::new();
    opener.insert(MemoryOpener::SAMPLE_PATH, repo);
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(opener));
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

fn run(registry: &Registry, session: &mut Session, choice: &Choice, file: IgnoreFile) -> Received {
    let mut b = IgnoreBox::default();
    let send = b.start(choice.pattern.clone(), file).unwrap();
    let out = registry.run(COMMAND, session, send);
    b.receive(out.map_err(|e| e.text()), None)
}

/// The full path with the `MemoryBackend`, as the UI does it: "All *.log files" in
/// `.gitignore`, the notice, the status without the logs, then undo.
#[test]
fn ignoring_an_extension_then_undo() {
    let (registry, mut session) = open();
    let extension = choices(&"debug.log".into()).remove(1);
    let Received::Done(result) = run(&registry, &mut session, &extension, IgnoreFile::Gitignore)
    else {
        panic!()
    };
    assert_eq!(
        notice(&result),
        "Added “*.log” to .gitignore; 2 untracked files are now ignored. “Undo” removes it"
    );
    let status = serde_json::to_value(&result).unwrap();
    let after = lists(&status);
    assert!(!after.contains(Side::Unstaged, &"debug.log".into()));
    assert!(!after.contains(Side::Unstaged, &"build/out.log".into()));
    assert!(after.contains(Side::Unstaged, &".gitignore".into()));
    let history = History::from(result.journal);
    assert!(history.can_undo);

    // The same pattern again: nothing written.
    let Received::Done(again) = run(&registry, &mut session, &extension, IgnoreFile::Gitignore)
    else {
        panic!()
    };
    assert_eq!(
        notice(&again),
        "“*.log” is already in .gitignore: nothing changed"
    );

    // Undo: the files come back.
    let out = registry
        .run(UndoOp::Undo.command(), &mut session, json!({}))
        .unwrap();
    assert!(lists(&out).contains(Side::Unstaged, &"debug.log".into()));
    assert!(!lists(&out).contains(Side::Unstaged, &".gitignore".into()));
}

/// "Only for me": `.git/info/exclude`, the worktree does not change.
#[test]
fn ignoring_a_folder_only_for_me() {
    let (registry, mut session) = open();
    let folder = choices(&"build/out.log".into()).remove(2);
    assert_eq!(folder.label, "Folder build/");
    let Received::Done(result) = run(&registry, &mut session, &folder, IgnoreFile::Exclude) else {
        panic!()
    };
    assert_eq!(
        notice(&result),
        "Added “/build/” to .git/info/exclude; 1 untracked file is now ignored. “Undo” removes it"
    );
    let after = lists(&serde_json::to_value(&result).unwrap());
    assert!(!after.contains(Side::Unstaged, &"build/out.log".into()));
    assert!(!after.contains(Side::Unstaged, &".gitignore".into()));
    assert!(after.contains(Side::Unstaged, &"debug.log".into()));
}

/// A pattern that matches tracked files: the warning is in the notice.
#[test]
fn tracked_files_matched_are_a_warning() {
    let (registry, mut session) = open();
    let choice = Choice {
        label: String::new(),
        pattern: "*.rs".into(),
    };
    let Received::Done(result) = run(&registry, &mut session, &choice, IgnoreFile::Gitignore)
    else {
        panic!()
    };
    let notice = notice(&result);
    assert!(
        notice.contains("no untracked file matched") && notice.contains("tracked files match"),
        "{notice}"
    );
}
