//! Git LFS on the `MemoryBackend`: the files stored in LFS known after each status (one
//! `repo.lfs_paths` only when the repo uses LFS), the diff of an LFS file shown as "Stored in Git
//! LFS (`<size>`)" without line operations, the notice "git-lfs is not installed" once per repo.

use super::*;
use crate::diff::{DiffSource, DiffTarget, Pane, Row};
use crate::executor::Executor;
use crate::lines::{self, Partial};
use crate::{Action as TabAction, RepoView};
use project_domain::{Change, EntryKind, FileDiff, Hunk, Line, Oid, StatusEntry, TrackedInfo};
use project_engine::{Capabilities, MemoryBackend, MemoryOpener, Platform, Registry, Session};
use std::sync::Arc;

/// The job runs inside `spawn`, on the same thread.
struct Inline;

impl Platform for Inline {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
        on_done(None);
    }
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        job();
    }
}

fn oid(n: u8) -> Oid {
    Oid::from_bytes([n; Oid::LEN])
}

fn pointer(size: u64) -> String {
    format!(
        "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize {size}\n",
        "ab".repeat(32)
    )
}

/// `a.txt` and `big.bin` modified in the worktree; `lfs`: `Some(installed)` stores `big.bin` in
/// LFS.
fn backend(lfs: Option<bool>) -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    repo.set_blob(oid(7), "one\n");
    repo.set_blob(oid(8), pointer(10));
    repo.set_worktree_file("a.txt", "two\n");
    repo.set_worktree_file("big.bin", pointer(1536));
    let entry = |path: &str, blob: u8| StatusEntry {
        path: RepoPath::from(path),
        index: Change::Unmodified,
        worktree: Change::Modified,
        kind: EntryKind::Ordinary(TrackedInfo {
            submodule: None,
            mode_head: 0o100644,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: oid(blob),
            oid_index: oid(blob),
        }),
    };
    repo.set_entries(vec![entry("a.txt", 7), entry("big.bin", 8)]);
    if let Some(installed) = lfs {
        repo.set_lfs([RepoPath::from("big.bin")], installed);
    }
    repo
}

fn tab_with(repo: MemoryBackend) -> Tab {
    let mut opener = MemoryOpener::new();
    opener.insert("repo", repo);
    let executor = Executor::new(
        Session::new(Arc::new(opener)),
        Registry::builtin(),
        Arc::new(Inline),
        Arc::new(|| {}),
    );
    let mut tab = Tab::new(0, executor);
    tab.open("repo".to_owned());
    settle(&mut tab);
    tab
}

fn settle(tab: &mut Tab) {
    while tab.executor.is_busy() {
        tab.receive();
    }
    tab.receive();
}

fn repo(tab: &Tab) -> &RepoView {
    tab.repo.as_ref().expect("open repo")
}

fn open_diff(tab: &mut Tab, path: &str) {
    let target = DiffTarget {
        source: DiffSource::Worktree,
        path: RepoPath::from(path),
    };
    tab.apply(TabAction::Diff(crate::diff::Action::Open(target)));
    settle(tab);
}

fn notes(tab: &Tab) -> Vec<String> {
    let Pane::Loaded(_, doc) = repo(tab).diff.pane() else {
        panic!("diff not loaded");
    };
    doc.rows
        .iter()
        .filter_map(|r| match r {
            Row::Note(_, note) => Some(note.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn lfs_files_are_known_after_the_status() {
    let tab = tab_with(backend(Some(true)));
    let lfs = &repo(&tab).lfs;
    assert!(lfs.used());
    assert!(lfs.is_lfs(&RepoPath::from("big.bin")));
    assert!(!lfs.is_lfs(&RepoPath::from("a.txt")));
    assert!(lfs.sent.is_empty() && lfs.asking.is_empty());
    assert!(!lfs.notice(), "git-lfs is installed");
    // The status was asked once: a new one is not sent again.
    assert_eq!(lfs.asked_status, Some(repo(&tab).status_seq));
}

#[test]
fn without_lfs_no_paths_are_asked() {
    let mut tab = tab_with(backend(None));
    let lfs = &repo(&tab).lfs;
    assert!(!lfs.used());
    assert!(lfs.known.is_empty() && lfs.asked_status.is_none());
    open_diff(&mut tab, "big.bin");
    let Pane::Loaded(_, doc) = repo(&tab).diff.pane() else {
        panic!("diff not loaded");
    };
    assert!(!doc.lfs);
    assert!(repo(&tab).lfs.known.is_empty());
}

#[test]
fn the_diff_of_an_lfs_file_shows_its_size_and_no_lines() {
    let mut tab = tab_with(backend(Some(true)));
    open_diff(&mut tab, "big.bin");
    assert_eq!(notes(&tab), ["Stored in Git LFS (1.5 KB)"]);
    let Pane::Loaded(target, doc) = repo(&tab).diff.pane() else {
        panic!("diff not loaded");
    };
    assert!(doc.lfs);
    assert!(
        !doc.rows
            .iter()
            .any(|r| matches!(r, Row::Hunk(..) | Row::Line(..)))
    );
    assert_eq!(lines::partial(doc, target.source), Partial::Hidden);
    assert_eq!(doc.line_counts(), (0, 0));

    // Another file: the pointer view is only for LFS files.
    open_diff(&mut tab, "a.txt");
    let Pane::Loaded(target, doc) = repo(&tab).diff.pane() else {
        panic!("diff not loaded");
    };
    assert!(!doc.lfs);
    assert!(matches!(
        lines::partial(doc, target.source),
        Partial::Available(_)
    ));
}

#[test]
fn a_diff_path_not_in_the_status_is_asked_once() {
    let mut b = LfsBox::default();
    b.receive_state(
        LfsState {
            used: true,
            installed: true,
        },
        true,
    );
    let path = RepoPath::from("old.bin");
    assert_eq!(b.lookup(&path), None);
    assert_eq!(b.diff_path(&path), Some(path.clone()));
    assert_eq!(b.diff_path(&path), None, "already on its way");
    b.sent.insert(Ticket::for_test(1), vec![path.clone()]);
    b.receive_paths(Ticket::for_test(1), Some(std::slice::from_ref(&path)));
    assert_eq!(b.lookup(&path), Some(true));
    assert_eq!(b.diff_path(&path), None, "known");
    // An error counts as "not in LFS": no request in a loop.
    let other = RepoPath::from("x.bin");
    assert_eq!(b.diff_path(&other), Some(other.clone()));
    b.sent.insert(Ticket::for_test(2), vec![other.clone()]);
    b.receive_paths(Ticket::for_test(2), None);
    assert_eq!(b.lookup(&other), Some(false));
    assert_eq!(b.diff_path(&other), None);
}

#[test]
fn the_missing_git_lfs_notice_is_shown_once_per_repo() {
    let mut tab = tab_with(backend(Some(false)));
    assert!(repo(&tab).lfs.notice());
    tab.apply(TabAction::Lfs(Action::Dismiss));
    assert!(!repo(&tab).lfs.notice());
    // "Refresh" and opening the repo again in the tab: not shown again.
    tab.apply(TabAction::Refresh);
    settle(&mut tab);
    assert!(!repo(&tab).lfs.notice());
    tab.open("repo".to_owned());
    settle(&mut tab);
    assert!(!repo(&tab).lfs.notice());
    // Installed or not used: never.
    assert!(!repo(&tab_with(backend(None))).lfs.notice());
}

fn line(kind: LineKind, text: &str) -> Line {
    Line {
        kind,
        old_line: None,
        new_line: None,
        content: project_domain::ByteText::from(text),
        no_newline: false,
    }
}

fn pointer_file(change: Change, lines: Vec<Line>) -> FileDiff {
    FileDiff {
        old_path: RepoPath::from("a.bin"),
        new_path: RepoPath::from("a.bin"),
        change,
        old_mode: Some(0o100644),
        new_mode: Some(0o100644),
        old_oid: None,
        new_oid: None,
        similarity: None,
        binary: false,
        hunks: vec![Hunk {
            old_start: 1,
            old_count: 3,
            new_start: 1,
            new_count: 3,
            section: project_domain::ByteText::from(""),
            lines,
        }],
    }
}

#[test]
fn the_note_reads_the_size_of_the_pointer() {
    let modified = pointer_file(
        Change::Modified,
        vec![
            line(
                LineKind::Context,
                "version https://git-lfs.github.com/spec/v1",
            ),
            line(LineKind::Removed, "size 10"),
            line(LineKind::Added, "size 3145728"),
        ],
    );
    assert_eq!(note(&modified), "Stored in Git LFS (3.0 MB)");
    let deleted = pointer_file(Change::Deleted, vec![line(LineKind::Removed, "size 1")]);
    assert_eq!(note(&deleted), "Stored in Git LFS (1 byte)");
    let broken = pointer_file(Change::Modified, vec![line(LineKind::Added, "size x")]);
    assert_eq!(note(&broken), "Stored in Git LFS");
    assert_eq!(size_text(1023), "1023 bytes");
    assert_eq!(size_text(5 * 1024 * 1024 * 1024), "5.0 GB");
}
