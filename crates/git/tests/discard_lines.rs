//! Integration tests of `worktree.discard_lines` with the `CliBackend` on real repos: one line of
//! a hunk, a whole hunk, added/removed/modified lines, end of file without newline, CRLF with
//! `core.autocrlf`, `clean`/`smudge` filters, executable bit kept, file changed after the read,
//! stale digest, backup failure, undo/redo and a file changed before the undo. The index is
//! never touched.

mod common;

use common::Repo;
use project_domain::{FileDiff, LineRef};
use project_engine::{EngineError, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn open(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn bytes(repo: &Repo, file: &str) -> Vec<u8> {
    std::fs::read(repo.path().join(file)).unwrap()
}

/// The file of `diff.worktree` for `path` and its digest.
fn seen(r: &Registry, s: &mut Session, path: &str) -> (FileDiff, String) {
    let diff = run(r, s, "diff.worktree", json!({ "path": path }));
    let file = diff["files"][0].clone();
    let digest = file["digest"].as_str().unwrap().to_owned();
    (serde_json::from_value(file).unwrap(), digest)
}

/// `worktree.discard_lines` of the lines `pick` chooses in the current diff.
fn discard(
    r: &Registry,
    s: &mut Session,
    path: &str,
    pick: impl FnOnce(&FileDiff) -> Vec<LineRef>,
) -> Result<Value, EngineError> {
    let (file, digest) = seen(r, s, path);
    let lines = pick(&file);
    r.run(
        "worktree.discard_lines",
        s,
        json!({ "path": path, "lines": lines, "digest": digest, "confirm": true }),
    )
}

/// A repo with `f.txt` committed (`base`) and rewritten in the worktree (`now`).
fn repo_with(base: &[u8], now: &[u8]) -> Repo {
    let repo = Repo::new();
    std::fs::write(repo.path().join("f.txt"), base).unwrap();
    repo.stage(&["f.txt"]);
    repo.commit("first");
    std::fs::write(repo.path().join("f.txt"), now).unwrap();
    repo
}

fn backups(repo: &Repo) -> String {
    repo.git(&["for-each-ref", "refs/project/backup/"])
}

const BASE: &str =
    "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\nmore\nand\nmore\ntwelve\n";
/// `two` modified, `four` removed, `x` added after `five`; then far away `twelve` modified.
const NOW: &str =
    "one\nTWO\nthree\nfive\nx\nsix\nseven\neight\nnine\nten\neleven\nmore\nand\nmore\nTWELVE\n";

#[test]
fn one_line_then_undo_and_redo() {
    let repo = repo_with(BASE.as_bytes(), NOW.as_bytes());
    let index_before = repo.git(&["ls-files", "-s"]);
    let (r, mut s) = open(&repo);

    // Without confirm: nothing written, no backup.
    let (_, digest) = seen(&r, &mut s, "f.txt");
    let err = r
        .run(
            "worktree.discard_lines",
            &mut s,
            json!({ "path": "f.txt", "lines": [{ "new": 5 }], "digest": digest }),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::ConfirmRequired {
                command: "worktree.discard_lines",
                ..
            }
        ),
        "{err}"
    );
    assert!(err.to_string().contains("1 changed line"), "{err}");
    assert_eq!(bytes(&repo, "f.txt"), NOW.as_bytes());
    assert_eq!(backups(&repo), "");

    // The added line `x` only.
    let out = discard(&r, &mut s, "f.txt", |_| vec![LineRef::New(5)]).unwrap();
    let after = NOW.replace("five\nx\n", "five\n");
    assert_eq!(bytes(&repo, "f.txt"), after.as_bytes());
    assert_eq!(repo.git(&["ls-files", "-s"]), index_before);
    let backup = out["backup"].as_str().unwrap();
    assert!(
        backup.starts_with("refs/project/backup/discard-"),
        "{backup}"
    );
    assert_eq!(
        repo.git(&["cat-file", "blob", &format!("{backup}:f.txt")]),
        NOW.trim_end()
    );
    // The parent of the backup has the bytes written (reachable for the redo).
    assert_eq!(
        repo.git(&["cat-file", "blob", &format!("{backup}^:f.txt")]),
        after.trim_end()
    );
    assert_eq!(out["path"], "f.txt");
    assert_eq!(out["mode"], 0o100644);
    assert_eq!(out["warnings"], json!([]));
    assert!(out["can_undo"].as_bool().unwrap());
    // The backup survives the garbage collector.
    repo.git(&["gc", "--prune=now", "--quiet"]);
    repo.git(&["cat-file", "-e", &format!("{backup}:f.txt")]);

    // Undo: the bytes before; redo: the bytes written; no new backup.
    let refs = backups(&repo);
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(bytes(&repo, "f.txt"), NOW.as_bytes());
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(bytes(&repo, "f.txt"), after.as_bytes());
    assert_eq!(backups(&repo), refs);

    // Changed before the undo: conflict, nothing written.
    repo.write("f.txt", "other\n");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(bytes(&repo, "f.txt"), b"other\n");
}

#[test]
fn whole_hunk_and_kinds_of_lines() {
    let repo = repo_with(BASE.as_bytes(), NOW.as_bytes());
    let (r, mut s) = open(&repo);
    // The first hunk whole (modified `two`, removed `four`, added `x`): the second stays.
    discard(&r, &mut s, "f.txt", |f| f.hunks[0].line_refs()).unwrap();
    assert_eq!(
        String::from_utf8(bytes(&repo, "f.txt")).unwrap(),
        BASE.replace("twelve\n", "TWELVE\n")
    );
    // The removed line of a modified one: `TWELVE` stays and `twelve` comes back before it.
    discard(&r, &mut s, "f.txt", |f| vec![f.hunks[0].line_refs()[0]]).unwrap();
    assert_eq!(
        String::from_utf8(bytes(&repo, "f.txt")).unwrap(),
        BASE.replace("twelve\n", "twelve\nTWELVE\n")
    );
    // The remaining added line: the file equals the index.
    discard(&r, &mut s, "f.txt", FileDiff::line_refs).unwrap();
    assert_eq!(bytes(&repo, "f.txt"), BASE.as_bytes());
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    // Every discard went into the journal.
    for _ in 0..3 {
        run(&r, &mut s, "edit.undo", json!({}));
    }
    assert_eq!(bytes(&repo, "f.txt"), NOW.as_bytes());
}

#[test]
fn removed_line_comes_back() {
    let repo = repo_with(BASE.as_bytes(), NOW.as_bytes());
    let (r, mut s) = open(&repo);
    discard(&r, &mut s, "f.txt", |_| vec![LineRef::Old(4)]).unwrap();
    assert_eq!(
        String::from_utf8(bytes(&repo, "f.txt")).unwrap(),
        NOW.replace("three\n", "three\nfour\n")
    );
}

#[test]
fn end_of_file_without_newline() {
    let repo = repo_with(b"a\nb\nc", b"a\nb\nc\nd");
    let (r, mut s) = open(&repo);
    // Only the added `d`: `c` keeps the newline it gained.
    discard(&r, &mut s, "f.txt", |_| vec![LineRef::New(4)]).unwrap();
    assert_eq!(bytes(&repo, "f.txt"), b"a\nb\nc\n");
    discard(&r, &mut s, "f.txt", FileDiff::line_refs).unwrap();
    assert_eq!(bytes(&repo, "f.txt"), b"a\nb\nc");
}

#[test]
fn crlf_with_autocrlf_true() {
    let repo = Repo::new();
    repo.git(&["config", "core.autocrlf", "true"]);
    std::fs::write(repo.path().join("f.txt"), b"a\r\nb\r\nc\r\n").unwrap();
    repo.stage(&["f.txt"]);
    repo.commit("first");
    std::fs::write(repo.path().join("f.txt"), b"a\r\nB\r\nc\r\nd\r\n").unwrap();
    let (r, mut s) = open(&repo);
    discard(&r, &mut s, "f.txt", |_| vec![LineRef::New(4)]).unwrap();
    assert_eq!(bytes(&repo, "f.txt"), b"a\r\nB\r\nc\r\n");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(bytes(&repo, "f.txt"), b"a\r\nB\r\nc\r\nd\r\n");
}

#[test]
fn bytes_that_do_not_round_trip_are_refused() {
    // `core.autocrlf=input`: CRLF on disk is cleaned to LF and never smudged back.
    let repo = repo_with(b"a\nb\n", b"a\r\nB\r\n");
    repo.git(&["config", "core.autocrlf", "input"]);
    let (r, mut s) = open(&repo);
    let err = discard(&r, &mut s, "f.txt", |_| vec![LineRef::New(2)]).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(bytes(&repo, "f.txt"), b"a\r\nB\r\n");
    assert_eq!(backups(&repo), "");
}

#[cfg(unix)]
#[test]
fn clean_and_smudge_filter() {
    // In the repo uppercase, in the worktree lowercase.
    let repo = Repo::new();
    repo.git(&["config", "filter.case.clean", "tr a-z A-Z"]);
    repo.git(&["config", "filter.case.smudge", "tr A-Z a-z"]);
    repo.write(".gitattributes", "f.txt filter=case\n");
    repo.write("f.txt", "a\nb\nc\n");
    repo.stage(&[".gitattributes", "f.txt"]);
    repo.commit("first");
    repo.write("f.txt", "a\nx\nc\ny\n");
    let (r, mut s) = open(&repo);
    let (file, _) = seen(&r, &mut s, "f.txt");
    assert!(
        file.hunks[0]
            .lines
            .iter()
            .any(|l| l.content.as_bytes() == b"X")
    );
    discard(&r, &mut s, "f.txt", |_| vec![LineRef::New(4)]).unwrap();
    assert_eq!(bytes(&repo, "f.txt"), b"a\nx\nc\n");
    assert_eq!(repo.git(&["cat-file", "blob", ":f.txt"]), "A\nB\nC");
}

#[cfg(unix)]
#[test]
fn executable_bit_is_kept() {
    use std::os::unix::fs::PermissionsExt;
    let repo = repo_with(BASE.as_bytes(), NOW.as_bytes());
    let path = repo.path().join("f.txt");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (r, mut s) = open(&repo);
    let out = discard(&r, &mut s, "f.txt", |_| vec![LineRef::New(5)]).unwrap();
    assert_eq!(out["mode"], 0o100755);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
    // The backup has the mode too.
    let backup = out["backup"].as_str().unwrap();
    assert!(repo.git(&["ls-tree", backup]).starts_with("100755 "));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

/// With `core.fileMode=false` git ignores the permissions on disk: an executable file whose index
/// entry is `100644` can be discarded (and undone), and the mode recorded is git's.
#[cfg(unix)]
#[test]
fn file_mode_false_ignores_the_permissions_on_disk() {
    use std::os::unix::fs::PermissionsExt;
    let repo = repo_with(BASE.as_bytes(), NOW.as_bytes());
    repo.git(&["config", "core.fileMode", "false"]);
    let path = repo.path().join("f.txt");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (r, mut s) = open(&repo);
    let out = discard(&r, &mut s, "f.txt", |_| vec![LineRef::New(5)]).unwrap();
    assert_eq!(out["mode"], 0o100644);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o755
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(bytes(&repo, "f.txt"), NOW.as_bytes());
    run(&r, &mut s, "edit.redo", json!({}));
    assert_ne!(bytes(&repo, "f.txt"), NOW.as_bytes());
}

#[test]
fn stale_digest_writes_nothing() {
    let repo = repo_with(BASE.as_bytes(), NOW.as_bytes());
    let (r, mut s) = open(&repo);
    let (_, digest) = seen(&r, &mut s, "f.txt");
    repo.write("f.txt", &NOW.replace("five\nx\n", "five\ny\n"));
    let err = r
        .run(
            "worktree.discard_lines",
            &mut s,
            json!({ "path": "f.txt", "lines": [{ "new": 5 }], "digest": digest, "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
    assert_eq!(
        String::from_utf8(bytes(&repo, "f.txt")).unwrap(),
        NOW.replace("five\nx\n", "five\ny\n")
    );
    assert_eq!(backups(&repo), "");
}

#[cfg(unix)]
#[test]
fn file_changed_after_the_read_writes_nothing() {
    // A smudge filter that, the second time it runs (when the bytes to write are computed),
    // appends a line to the file: the last check before writing must notice.
    let repo = Repo::new();
    let script = repo.path().join(".git/late.sh");
    std::fs::write(
        &script,
        "cat\nn=$(cat .git/late-count 2>/dev/null || echo 0); n=$((n+1)); echo $n > .git/late-count\n\
         if [ $n = 2 ]; then printf 'late\\n' >> \"$1\"; fi\n",
    )
    .unwrap();
    repo.git(&["config", "filter.late.clean", "cat"]);
    repo.git(&[
        "config",
        "filter.late.smudge",
        &format!("sh {} %f", script.display()),
    ]);
    repo.write(".gitattributes", "f.txt filter=late\n");
    repo.write("f.txt", BASE);
    repo.stage(&[".gitattributes", "f.txt"]);
    repo.commit("first");
    let _ = std::fs::remove_file(repo.path().join(".git/late-count"));
    repo.write("f.txt", NOW);
    let (r, mut s) = open(&repo);
    let err = discard(&r, &mut s, "f.txt", |_| vec![LineRef::New(5)]).unwrap_err();
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
    assert_eq!(
        String::from_utf8(bytes(&repo, "f.txt")).unwrap(),
        format!("{NOW}late\n")
    );
    assert!(!s.journal().unwrap().can_undo());
}

#[test]
fn backup_failure_writes_nothing() {
    let repo = repo_with(BASE.as_bytes(), NOW.as_bytes());
    std::fs::create_dir_all(repo.path().join(".git/refs/project")).unwrap();
    std::fs::write(repo.path().join(".git/refs/project/backup"), "x").unwrap();
    let (r, mut s) = open(&repo);
    let err = discard(&r, &mut s, "f.txt", |_| vec![LineRef::New(5)]).unwrap_err();
    assert!(!matches!(err, EngineError::StaleDiff { .. }), "{err}");
    assert_eq!(bytes(&repo, "f.txt"), NOW.as_bytes());
}

#[test]
fn refusals() {
    let repo = repo_with(BASE.as_bytes(), NOW.as_bytes());
    repo.write("new.txt", "untracked\n");
    repo.write("-d ash.txt", "a\n");
    repo.stage(&["-d ash.txt"]);
    repo.commit("second");
    std::fs::remove_file(repo.path().join("-d ash.txt")).unwrap();
    let (r, mut s) = open(&repo);
    for (path, lines) in [
        ("new.txt", json!([{ "new": 1 }])),
        ("-d ash.txt", json!([{ "old": 1 }])),
    ] {
        let err = r
            .run(
                "worktree.discard_lines",
                &mut s,
                json!({ "path": path, "lines": lines, "digest": "0", "confirm": true }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{path}: {err}");
    }
    // A context line.
    let err = discard(&r, &mut s, "f.txt", |_| vec![LineRef::New(1)]).unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    assert_eq!(bytes(&repo, "f.txt"), NOW.as_bytes());
    assert_eq!(backups(&repo), "");
}

#[test]
fn name_with_dash_and_space() {
    let repo = Repo::new();
    repo.write("-x y.txt", BASE);
    repo.stage(&["-x y.txt"]);
    repo.commit("first");
    repo.write("-x y.txt", NOW);
    let (r, mut s) = open(&repo);
    discard(&r, &mut s, "-x y.txt", |_| vec![LineRef::New(5)]).unwrap();
    assert_eq!(
        String::from_utf8(bytes(&repo, "-x y.txt")).unwrap(),
        NOW.replace("five\nx\n", "five\n")
    );
    // No temporary file left behind.
    assert_eq!(repo.git(&["status", "--porcelain"]), " M \"-x y.txt\"");
}
