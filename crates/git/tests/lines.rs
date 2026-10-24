//! Integration tests of `CliBackend::apply_lines` and of the `index.stage_lines` and
//! `index.unstage_lines` commands on real repos.
//!
//! The index content is read with `git cat-file blob :<path>`; the worktree must never
//! change, not even in the modification time of the files.

mod common;

use common::Repo;
use project_domain::{FileDiff, LineKind, LineRef, PatchDirection, RepoPath};
use project_engine::{
    DiffSpec, EngineError, GitBackend, LineSelection, Registry, Session, file_digest,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

#[cfg(unix)]
fn os(path: &[u8]) -> &OsStr {
    use std::os::unix::ffi::OsStrExt;
    OsStr::from_bytes(path)
}

#[cfg(not(unix))]
fn os(path: &[u8]) -> &OsStr {
    OsStr::new(std::str::from_utf8(path).unwrap())
}

/// Every file of the worktree (outside `.git`): content and modification time.
type Snapshot = BTreeMap<PathBuf, (Vec<u8>, SystemTime)>;

fn snapshot(repo: &Repo) -> Snapshot {
    fn walk(dir: &Path, root: &Path, out: &mut Snapshot) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.file_name() == Some(OsStr::new(".git")) {
                continue;
            }
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(
                    rel,
                    (std::fs::read(&path).unwrap(), meta.modified().unwrap()),
                );
            }
        }
    }
    let mut out = Snapshot::new();
    walk(repo.path(), repo.path(), &mut out);
    out
}

/// The content of `path` in the index (`git cat-file blob :<path>`); `None` if absent.
fn index(repo: &Repo, path: &[u8]) -> Option<Vec<u8>> {
    let mut spec = b":".to_vec();
    spec.extend_from_slice(path);
    let out = std::process::Command::new("git")
        .envs(common::GIT_ENV.iter().copied())
        .arg("-C")
        .arg(repo.path())
        .args(["cat-file", "blob"])
        .arg(os(&spec))
        .output()
        .unwrap();
    out.status.success().then_some(out.stdout)
}

fn index_str(repo: &Repo, path: &str) -> Option<String> {
    index(repo, path.as_bytes()).map(|b| String::from_utf8(b).unwrap())
}

/// The whole index (`git ls-files -s`), to check that it did not change.
fn listing(repo: &Repo) -> String {
    repo.git(&["ls-files", "-s"])
}

fn backups(repo: &Repo) -> Vec<String> {
    let out = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/project/backup/",
    ]);
    out.lines().map(str::to_owned).collect()
}

fn numbered(n: usize, changed: &[(usize, &str)]) -> String {
    (1..=n)
        .map(|i| match changed.iter().find(|(j, _)| *j == i) {
            Some((_, text)) => format!("{text}\n"),
            None => format!("line {i}\n"),
        })
        .collect()
}

/// The file `path` of the worktree diff (stage) or the index diff (unstage), as the UI sees it.
fn seen(repo: &Repo, path: &[u8], direction: PatchDirection) -> FileDiff {
    let path = Some(RepoPath::from_bytes(path));
    let spec = match direction {
        PatchDirection::Stage => DiffSpec::Worktree { path },
        PatchDirection::Unstage => DiffSpec::Index { path },
    };
    let mut diff = repo.open().diff(&spec).unwrap();
    assert_eq!(diff.files.len(), 1, "one file in the diff: {diff:?}");
    diff.files.remove(0)
}

/// The reference of the line of `file` with that prefix (`'-'` or `'+'`) and content.
fn line(file: &FileDiff, prefix: char, content: &[u8]) -> LineRef {
    let kind = if prefix == '-' {
        LineKind::Removed
    } else {
        LineKind::Added
    };
    let mut found = file
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == kind && l.content.as_bytes() == content);
    let l = found.next().expect("line present in the diff");
    assert!(found.next().is_none(), "unique line");
    l.line_ref().unwrap()
}

/// The selection of the lines `picks` of `path`, with the digest of the diff just read.
fn select(
    repo: &Repo,
    path: &[u8],
    direction: PatchDirection,
    picks: &[(char, &[u8])],
) -> LineSelection {
    let file = seen(repo, path, direction);
    LineSelection {
        path: RepoPath::from_bytes(path),
        lines: picks.iter().map(|(p, c)| line(&file, *p, c)).collect(),
        digest: file_digest(&file),
        direction,
    }
}

/// Applies the selection with the backend and checks that the worktree did not change.
fn apply(repo: &Repo, selection: &LineSelection) -> Result<Option<String>, EngineError> {
    let before = snapshot(repo);
    let result = repo.open().apply_lines(selection);
    assert_eq!(snapshot(repo), before, "the worktree does not change");
    result
}

fn stage(repo: &Repo, path: &str, picks: &[(char, &str)]) -> Option<String> {
    let picks: Vec<(char, &[u8])> = picks.iter().map(|(p, c)| (*p, c.as_bytes())).collect();
    let sel = select(repo, path.as_bytes(), PatchDirection::Stage, &picks);
    apply(repo, &sel).unwrap()
}

fn unstage(repo: &Repo, path: &str, picks: &[(char, &str)]) -> Option<String> {
    let picks: Vec<(char, &[u8])> = picks.iter().map(|(p, c)| (*p, c.as_bytes())).collect();
    let sel = select(repo, path.as_bytes(), PatchDirection::Unstage, &picks);
    apply(repo, &sel).unwrap()
}

/// Failed outcome: the error, with index, backup refs and worktree unchanged.
fn refused(repo: &Repo, selection: &LineSelection) -> EngineError {
    let (index_before, backups_before) = (listing(repo), backups(repo));
    let err = apply(repo, selection).unwrap_err();
    assert_eq!(
        listing(repo),
        index_before,
        "the index does not change: {err}"
    );
    assert_eq!(backups(repo), backups_before, "no backup: {err}");
    err
}

/// `(X, Y)` of `path` in the status.
fn letters(repo: &Repo, path: &str) -> (char, char) {
    let status = repo.open().status().unwrap();
    let e = status
        .entries
        .iter()
        .find(|e| e.path == RepoPath::from(path))
        .expect("entry in the status");
    (e.index.letter(), e.worktree.letter())
}

#[test]
fn hunk_scattered_lines_and_partial_unstage() {
    let repo = Repo::new();
    repo.commit_file("f.txt", &numbered(30, &[]), "base");
    let changed = numbered(
        30,
        &[(2, "TWO"), (3, "THREE"), (15, "FIFTEEN"), (29, "XXIX")],
    );
    repo.write("f.txt", &changed);
    assert_eq!(seen(&repo, b"f.txt", PatchDirection::Stage).hunks.len(), 3);

    // A whole hunk: the middle one.
    let file = seen(&repo, b"f.txt", PatchDirection::Stage);
    let sel = LineSelection {
        path: RepoPath::from("f.txt"),
        lines: file.hunks[1].line_refs(),
        digest: file_digest(&file),
        direction: PatchDirection::Stage,
    };
    assert_eq!(apply(&repo, &sel).unwrap(), None);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(30, &[(15, "FIFTEEN")])
    );
    assert_eq!(letters(&repo, "f.txt"), ('M', 'M'));

    // Scattered lines in two hunks: the staged version (`MM`) changes, a backup first.
    let backup = stage(
        &repo,
        "f.txt",
        &[
            ('-', "line 2"),
            ('+', "TWO"),
            ('+', "XXIX"),
            ('-', "line 29"),
        ],
    )
    .expect("backup of the staged version");
    assert_eq!(
        repo.git(&["show", &format!("{backup}:f.txt")]),
        numbered(30, &[(15, "FIFTEEN")]).trim_end()
    );
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(30, &[(2, "TWO"), (15, "FIFTEEN"), (29, "XXIX")])
    );

    // Unstage of a part, from the index diff.
    let backup = unstage(&repo, "f.txt", &[('-', "line 15"), ('+', "FIFTEEN")]);
    assert!(
        backup.is_some(),
        "here too the staged version was only in the index"
    );
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(30, &[(2, "TWO"), (29, "XXIX")])
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("f.txt")).unwrap(),
        changed
    );
    assert_eq!(backups(&repo).len(), 2);
}

#[test]
fn no_backup_if_the_staged_version_is_not_at_risk() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\nc\n", "base");
    repo.write("f.txt", "a\nB\nc\nd\n");
    // `.M`: the index equals HEAD.
    assert_eq!(stage(&repo, "f.txt", &[('+', "d")]), None);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nb\nc\nd\n");
    // Everything staged (`M.`): the index equals the worktree.
    repo.stage(&["f.txt"]);
    assert_eq!(unstage(&repo, "f.txt", &[('+', "d")]), None);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nB\nc\n");
    assert!(backups(&repo).is_empty());
}

#[test]
fn new_file_intent_to_add() {
    let repo = Repo::new();
    repo.commit_file("base.txt", "x\n", "base");
    repo.write("new.txt", "one\ntwo\nthree\n");
    repo.git(&["add", "-N", "new.txt"]);
    assert_eq!(letters(&repo, "new.txt"), ('.', 'A'));
    assert_eq!(
        stage(&repo, "new.txt", &[('+', "one"), ('+', "three")]),
        None
    );
    assert_eq!(index_str(&repo, "new.txt").unwrap(), "one\nthree\n");
    assert_eq!(letters(&repo, "new.txt"), ('A', 'M'));
    // And unstaging all the lines of a new file removes it from the index.
    let backup = unstage(&repo, "new.txt", &[('+', "one"), ('+', "three")]);
    assert!(
        backup.is_some(),
        "`AM`: the staged version was only in the index"
    );
    assert_eq!(index(&repo, b"new.txt"), None);
    assert_eq!(letters(&repo, "new.txt"), ('?', '?'));
}

#[test]
fn file_deleted_in_part_and_then_entirely() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\nc\n", "base");
    std::fs::remove_file(repo.path().join("f.txt")).unwrap();
    assert_eq!(stage(&repo, "f.txt", &[('-', "b")]), None);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nc\n");
    assert_eq!(letters(&repo, "f.txt"), ('M', 'D'));
    // The remaining lines: the file leaves the index. The staged version (`MD`) goes in the backup.
    let backup = stage(&repo, "f.txt", &[('-', "a"), ('-', "c")]).unwrap();
    assert_eq!(index(&repo, b"f.txt"), None);
    assert_eq!(letters(&repo, "f.txt"), ('D', '.'));
    assert_eq!(repo.git(&["show", &format!("{backup}:f.txt")]), "a\nc");
    // Unstaging a line of a file deleted in the index puts it back in the index.
    assert_eq!(unstage(&repo, "f.txt", &[('-', "b")]), None);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "b\n");
}

#[test]
fn end_of_file_without_newline() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb", "base");
    repo.write("f.txt", "a\nb\nc\nd");
    assert_eq!(stage(&repo, "f.txt", &[('+', "c")]), None);
    assert_eq!(index(&repo, b"f.txt").unwrap(), b"a\nb\nc\n");
    assert!(
        stage(&repo, "f.txt", &[('+', "d")]).is_some(),
        "`MM`: backup"
    );
    assert_eq!(index(&repo, b"f.txt").unwrap(), b"a\nb\nc\nd");
}

#[test]
fn crlf_byte_for_byte() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\r\nb\r\nc\r\n", "base");
    repo.write("f.txt", "a\r\nB\r\nc\r\nd\r\n");
    assert_eq!(stage(&repo, "f.txt", &[('+', "d\r")]), None);
    assert_eq!(index(&repo, b"f.txt").unwrap(), b"a\r\nb\r\nc\r\nd\r\n");
    stage(&repo, "f.txt", &[('-', "b\r"), ('+', "B\r")]);
    assert_eq!(index(&repo, b"f.txt").unwrap(), b"a\r\nB\r\nc\r\nd\r\n");
}

#[cfg(unix)]
#[test]
fn non_utf8_name_with_spaces() {
    let repo = Repo::new();
    let name: &[u8] = b"cafe\xff with space.txt";
    std::fs::write(repo.path().join(os(name)), "a\nb\n").unwrap();
    repo.git(&["add", "-A"]);
    repo.commit("base");
    std::fs::write(repo.path().join(os(name)), "a\nB\nc\n").unwrap();
    let sel = select(&repo, name, PatchDirection::Stage, &[('+', b"c")]);
    assert_eq!(apply(&repo, &sel).unwrap(), None);
    assert_eq!(index(&repo, name).unwrap(), b"a\nb\nc\n");
}

#[test]
fn user_configuration_neutralized() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\n", "base");
    repo.git(&["config", "apply.whitespace", "error"]);
    repo.git(&["config", "apply.ignoreWhitespace", "change"]);
    repo.git(&[
        "config",
        "core.whitespace",
        "trailing-space,space-before-tab",
    ]);
    repo.write("f.txt", "a\nb  \n \tc\n");
    stage(&repo, "f.txt", &[('+', "b  "), ('-', "b"), ('+', " \tc")]);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nb  \n \tc\n");
}

#[test]
fn invalid_selection() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\nc\n", "base");
    repo.write("f.txt", "a\nB\nc\n");
    let file = seen(&repo, b"f.txt", PatchDirection::Stage);
    for lines in [
        vec![LineRef::New(1)], // context
        vec![LineRef::Old(9)], // nonexistent
        vec![LineRef::Old(1)], // context, from the old side
        vec![LineRef::New(2), LineRef::New(7)],
        vec![],
    ] {
        let sel = LineSelection {
            path: RepoPath::from("f.txt"),
            lines,
            digest: file_digest(&file),
            direction: PatchDirection::Stage,
        };
        let err = refused(&repo, &sel);
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    }
    // No staged change: nothing to remove.
    let sel = LineSelection {
        direction: PatchDirection::Unstage,
        path: RepoPath::from("f.txt"),
        lines: vec![LineRef::New(2)],
        digest: file_digest(&file),
    };
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
}

#[test]
fn file_changed_between_diff_and_apply() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\nc\n", "base");
    repo.write("f.txt", "a\nB\nc\n");
    let sel = select(&repo, b"f.txt", PatchDirection::Stage, &[('+', b"B")]);

    // Same line numbers, different content in the worktree.
    repo.write("f.txt", "a\nX\nc\n");
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");

    // Index changed from outside (same worktree as when the diff was read).
    repo.write("f.txt", "a\nB\nc\n");
    let sel = select(&repo, b"f.txt", PatchDirection::Stage, &[('+', b"B")]);
    repo.write("f.txt", "a\nb\nc\nd\n");
    repo.stage(&["f.txt"]);
    repo.write("f.txt", "a\nB\nc\n");
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nb\nc\nd\n");

    // The file has no changes anymore.
    let sel = select(&repo, b"f.txt", PatchDirection::Stage, &[('+', b"B")]);
    repo.stage(&["f.txt"]);
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
}

#[test]
fn binary_and_submodule_refused() {
    let repo = Repo::new();
    std::fs::write(repo.path().join("bin.dat"), b"a\0b\n").unwrap();
    repo.stage(&["bin.dat"]);
    let head = repo.commit("base");
    std::fs::write(repo.path().join("bin.dat"), b"a\0c\n").unwrap();
    let file = seen(&repo, b"bin.dat", PatchDirection::Stage);
    assert!(file.binary);
    let sel = LineSelection {
        path: RepoPath::from("bin.dat"),
        lines: vec![LineRef::New(1)],
        digest: file_digest(&file),
        direction: PatchDirection::Stage,
    };
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    // A submodule (gitlink) changed in the index.
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},sub"),
    ]);
    let second = repo.commit("submodule");
    repo.git(&[
        "update-index",
        "--cacheinfo",
        &format!("160000,{second},sub"),
    ]);
    let file = seen(&repo, b"sub", PatchDirection::Unstage);
    assert!(file.is_submodule());
    let sel = LineSelection {
        path: RepoPath::from("sub"),
        lines: file.line_refs(),
        digest: file_digest(&file),
        direction: PatchDirection::Unstage,
    };
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
}

#[test]
fn locked_index() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\n", "base");
    repo.write("f.txt", "a\nB\n");
    repo.stage(&["f.txt"]);
    repo.write("f.txt", "a\nB\nc\n");
    // `MM`: a backup would be needed, but with the index locked nothing is saved.
    let sel = select(&repo, b"f.txt", PatchDirection::Stage, &[('+', b"c")]);
    let lock = repo.path().join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    let err = refused(&repo, &sel);
    let EngineError::IndexLocked { lock: reported } = &err else {
        panic!("{err}");
    };
    assert_eq!(Path::new(reported), lock);
    assert!(lock.exists(), "the lock is never deleted");
    std::fs::remove_file(&lock).unwrap();
    assert!(apply(&repo, &sel).unwrap().is_some());
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nB\nc\n");
}

#[test]
fn unstage_of_a_rename_by_old_path() {
    let repo = Repo::new();
    repo.commit_file("old.txt", "one\ntwo\nthree\nfour\n", "base");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.write("new.txt", "one\ntwo\nthree\nfour\nfive\n");
    repo.stage(&["new.txt"]);
    let sel = select(
        &repo,
        b"old.txt",
        PatchDirection::Unstage,
        &[('+', b"five")],
    );
    assert_eq!(apply(&repo, &sel).unwrap(), None);
    assert_eq!(
        index_str(&repo, "new.txt").unwrap(),
        "one\ntwo\nthree\nfour\n"
    );
    assert_eq!(index(&repo, b"old.txt"), None);
}

fn run(registry: &Registry, session: &mut Session, id: &str, params: Value) -> Value {
    registry
        .run(id, session, params)
        .unwrap_or_else(|e| panic!("{id}: {e}"))
}

#[test]
fn commands_on_the_real_repo() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\nc\n", "base");
    repo.write("f.txt", "a\nB\nc\nd\n");
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(Repo::opener()));
    run(
        &registry,
        &mut session,
        "repo.open",
        json!({ "path": repo.path_str() }),
    );
    let diff = run(
        &registry,
        &mut session,
        "diff.worktree",
        json!({ "path": "f.txt" }),
    );
    let file = &diff["files"][0];
    let digest = file["digest"].as_str().unwrap().to_owned();
    let parsed: FileDiff = serde_json::from_value(file.clone()).unwrap();
    assert_eq!(file_digest(&parsed), digest);

    let before = snapshot(&repo);
    let out = run(
        &registry,
        &mut session,
        "index.stage_lines",
        json!({ "path": "f.txt", "lines": [{ "new": 4 }], "digest": digest }),
    );
    assert_eq!(snapshot(&repo), before);
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["status"]["entries"][0]["index"], "Modified");
    assert_eq!(out["status"]["entries"][0]["worktree"], "Modified");
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nb\nc\nd\n");

    // The same digest is now stale.
    let err = registry
        .run(
            "index.stage_lines",
            &mut session,
            json!({ "path": "f.txt", "lines": [{ "new": 2 }], "digest": digest }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");

    let diff = run(
        &registry,
        &mut session,
        "diff.index",
        json!({ "path": "f.txt" }),
    );
    let digest = diff["files"][0]["digest"].clone();
    let out = run(
        &registry,
        &mut session,
        "index.unstage_lines",
        json!({ "path": "f.txt", "lines": [{ "new": 4 }], "digest": digest }),
    );
    // `MM`: the staged version was only in the index.
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(
        repo.git(&["show", &format!("{backup}:f.txt")]),
        "a\nb\nc\nd"
    );
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nb\nc\n");
    assert_eq!(out["status"]["entries"][0]["index"], "Unmodified");
}

/// `index.stage` (S18) of an `intent-to-add` file: `git status` gives mode and zero id for the
/// staged version, but the index has an entry; the recheck of the index before writing
/// must not mistake it for a concurrent change.
#[test]
fn whole_stage_of_an_intent_to_add_file() {
    let repo = Repo::new();
    repo.commit_file("base.txt", "x\n", "base");
    repo.write("new.txt", "one\n");
    repo.git(&["add", "-N", "new.txt"]);
    let mut backend = repo.open();
    assert_eq!(backend.stage(&[RepoPath::from("new.txt")]).unwrap(), None);
    assert_eq!(index_str(&repo, "new.txt").unwrap(), "one\n");
    assert_eq!(letters(&repo, "new.txt"), ('A', '.'));
}

/// With `diff.context=0` in the configuration the diff keeps three lines of context
/// (`--unified=3` in `DIFF_ARGS`) and the lines go in their place in the index, also in the
/// middle of the file and without a trailing newline (a hunk without context `git apply` would
/// anchor at the end).
#[test]
fn zero_context_in_the_configuration() {
    let repo = Repo::new();
    repo.git(&["config", "diff.context", "0"]);
    repo.commit_file("f.txt", "one\ntwo\nthree\nfour\n", "base");
    repo.write("f.txt", "one\ntwo\nNEW\nthree\nfour\n");
    let file = seen(&repo, b"f.txt", PatchDirection::Stage);
    assert_eq!(file.hunks[0].lines.len(), 5, "context around the insertion");
    assert_eq!(stage(&repo, "f.txt", &[('+', "NEW")]), None);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        "one\ntwo\nNEW\nthree\nfour\n"
    );
    // And the unstage of the same line, still with zero context.
    assert_eq!(unstage(&repo, "f.txt", &[('+', "NEW")]), None);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        "one\ntwo\nthree\nfour\n"
    );

    // Without a trailing newline the lines do not merge.
    repo.commit_file("g.txt", "one\ntwo\nthree", "no newline");
    repo.write("g.txt", "one\ntwo\nNEW\nthree");
    assert_eq!(stage(&repo, "g.txt", &[('+', "NEW")]), None);
    assert_eq!(index(&repo, b"g.txt").unwrap(), b"one\ntwo\nNEW\nthree");
    // Two distant insertions, only one chosen.
    repo.commit_file("h.txt", &numbered(20, &[]), "long");
    let mut changed = numbered(20, &[]).replace("line 3\n", "line 3\nFIRST\n");
    changed = changed.replace("line 17\n", "line 17\nSECOND\n");
    repo.write("h.txt", &changed);
    assert_eq!(stage(&repo, "h.txt", &[('+', "FIRST")]), None);
    assert_eq!(
        index_str(&repo, "h.txt").unwrap(),
        numbered(20, &[]).replace("line 3\n", "line 3\nFIRST\n")
    );
}

/// The letters of `git ls-files -v` for `path` (`H` normal, `S` skip-worktree, `h`
/// assume-unchanged).
fn tag(repo: &Repo, path: &str) -> String {
    repo.git(&["ls-files", "-v", "--", path])[..1].to_owned()
}

/// `update-index --index-info` and `restore --staged` recreate the entry: the flags
/// `--skip-worktree` and `--assume-unchanged` must be put back. A file outside the cone of a
/// sparse checkout (skip-worktree, absent from the worktree) with a staged change must not
/// look deleted after unstaging lines or the file.
#[test]
fn skip_worktree_and_assume_unchanged_stay() {
    let repo = Repo::new();
    repo.commit_file("d/f", "a\nb\nc\n", "base");
    repo.commit_file("u.txt", "a\nb\nc\n", "base u");
    repo.write("d/f", "a\nB\nc\nd\n");
    repo.write("u.txt", "a\nB\nc\nd\n");
    repo.stage(&["d/f", "u.txt"]);
    repo.git(&["update-index", "--skip-worktree", "d/f"]);
    repo.git(&["update-index", "--assume-unchanged", "u.txt"]);
    std::fs::remove_file(repo.path().join("d/f")).unwrap();
    assert_eq!(
        (tag(&repo, "d/f"), tag(&repo, "u.txt")),
        ("S".into(), "h".into())
    );
    assert_eq!(letters(&repo, "d/f"), ('M', '.'));

    // Unstage of a line: the staged versions are not visible in the worktree, backup.
    assert!(unstage(&repo, "d/f", &[('+', "d")]).is_some());
    assert!(unstage(&repo, "u.txt", &[('+', "d")]).is_some());
    assert_eq!(index_str(&repo, "d/f").unwrap(), "a\nB\nc\n");
    assert_eq!(index_str(&repo, "u.txt").unwrap(), "a\nB\nc\n");
    assert_eq!(
        (tag(&repo, "d/f"), tag(&repo, "u.txt")),
        ("S".into(), "h".into())
    );
    assert_eq!(letters(&repo, "d/f"), ('M', '.'), "does not look deleted");

    // Also `index.unstage` (S18, `restore --staged`).
    let mut backend = repo.open();
    backend
        .unstage(&[RepoPath::from("d"), RepoPath::from("u.txt")])
        .unwrap();
    assert_eq!(index_str(&repo, "d/f").unwrap(), "a\nb\nc\n");
    assert_eq!(
        (tag(&repo, "d/f"), tag(&repo, "u.txt")),
        ("S".into(), "h".into())
    );
    let status = repo.open().status().unwrap();
    assert!(
        !status
            .entries
            .iter()
            .any(|e| e.path == RepoPath::from("d/f")),
        "{status:?}"
    );
}
