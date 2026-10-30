//! Integration tests of `CliBackend::blame`/`file_log` and of the `file.blame`/`file.log`
//! commands on the scenarios of `common`.

mod common;

use common::{
    NON_UTF8_NAME, Repo, repo_with_file_changes, repo_with_renamed_file, repo_with_special_names,
};
use project_domain::{Change, FileLogEntry, Oid, RepoPath};
use project_engine::{
    BlameQuery, CancelToken, EngineError, FileBlame, FileLogQuery, GitRead, JobContext, Registry,
    Session,
};
use serde_json::json;
use std::sync::Arc;

fn blame(
    repo: &Repo,
    path: impl Into<RepoPath>,
    rev: Option<&str>,
) -> Result<FileBlame, EngineError> {
    let query = BlameQuery {
        path: path.into(),
        rev: rev.map(str::to_owned),
    };
    repo.open().blame(&query, &JobContext::new())
}

/// For each line: (text, id of its commit, orig_line).
fn lines(b: &FileBlame) -> Vec<(String, Option<Oid>, u32)> {
    b.blame
        .lines
        .iter()
        .map(|l| {
            (
                l.text.to_string(),
                l.commit.map(|i| b.blame.commits[i].id),
                l.orig_line,
            )
        })
        .collect()
}

fn line(text: &str, commit: Option<Oid>, orig: u32) -> (String, Option<Oid>, u32) {
    (text.to_owned(), commit, orig)
}

fn log(repo: &Repo, path: impl Into<RepoPath>, follow: bool) -> Vec<FileLogEntry> {
    let query = FileLogQuery {
        path: path.into(),
        rev: None,
        skip: 0,
        limit: None,
        follow,
    };
    repo.open().file_log(&query, &JobContext::new()).unwrap()
}

/// (commit, path, old path, status) of every entry.
fn summary(entries: &[FileLogEntry]) -> Vec<(Oid, String, Option<String>, Change)> {
    entries
        .iter()
        .map(|e| {
            (
                e.commit.id,
                e.path.to_string(),
                e.old_path.as_ref().map(ToString::to_string),
                e.status,
            )
        })
        .collect()
}

#[test]
fn blame_at_head_across_rename_and_merge() {
    let h = repo_with_renamed_file();
    let b = blame(&h.repo, "b.txt", Some("HEAD")).unwrap();
    assert_eq!(b.rev, Some(h.merge));
    assert_eq!(b.path, RepoPath::from("b.txt"));
    assert_eq!(
        lines(&b),
        [
            line("ONE", Some(h.one), 1),
            line("TWO", Some(h.two), 2),
            line("three", Some(h.add), 3),
            line("four", Some(h.four), 4),
            line("five", Some(h.five), 5),
        ]
    );
    let add = b.blame.commits.iter().find(|c| c.id == h.add).unwrap();
    assert_eq!(add.path, RepoPath::from("a.txt"));
    assert!(add.boundary);
    assert_eq!(add.previous, None);
    let four = b.blame.commits.iter().find(|c| c.id == h.four).unwrap();
    assert_eq!(four.previous, Some((h.rename, RepoPath::from("b.txt"))));
    assert_eq!(four.author.name, "Ada");
    assert_eq!(four.committer.name, "Bob");
    assert_eq!(four.summary, "four");
}

#[test]
fn blame_of_the_worktree_and_at_an_old_rev() {
    let h = repo_with_renamed_file();
    h.repo.write("b.txt", "zero\nONE\nTWO\nthree\nfour\nfive\n");
    let b = blame(&h.repo, "b.txt", None).unwrap();
    assert_eq!(b.rev, None);
    let got = lines(&b);
    assert_eq!(got[0], line("zero", None, 1));
    assert_eq!(got[1], line("ONE", Some(h.one), 1));
    assert_eq!(got.len(), 6);
    // A staged file not yet in HEAD: all uncommitted.
    h.repo.write("new.txt", "n\n");
    h.repo.stage(&["new.txt"]);
    let b = blame(&h.repo, "new.txt", None).unwrap();
    assert_eq!(lines(&b), [line("n", None, 1)]);
    // At an old commit, by abbreviated id, with the old name.
    let short = h.two.to_string()[..8].to_owned();
    let b = blame(&h.repo, "a.txt", Some(&short)).unwrap();
    assert_eq!(b.rev, Some(h.two));
    assert_eq!(
        lines(&b),
        [
            line("one", Some(h.add), 1),
            line("TWO", Some(h.two), 2),
            line("three", Some(h.add), 3),
        ]
    );
}

#[test]
fn blame_of_what_is_not_a_file() {
    let h = repo_with_renamed_file();
    let invalid = |r: Result<FileBlame, EngineError>| {
        let e = r.unwrap_err();
        assert!(matches!(e, EngineError::InvalidParams { .. }), "{e}");
        e.to_string()
    };
    let e = invalid(blame(&h.repo, "b.txt", Some(&h.two.to_string())));
    assert!(e.contains("b.txt") && e.contains(&h.two.to_string()), "{e}");
    invalid(blame(&h.repo, "nope.txt", Some("HEAD")));
    let e = invalid(blame(&h.repo, "nope.txt", None));
    assert!(e.contains("nope.txt"), "{e}");
    h.repo.write("untracked.txt", "u\n");
    invalid(blame(&h.repo, "untracked.txt", None));
    h.repo.commit_file("dir/x.txt", "x\n", "dir");
    invalid(blame(&h.repo, "dir", None));
    invalid(blame(&h.repo, "dir", Some("HEAD")));
    // Deleted from the worktree only.
    std::fs::remove_file(h.repo.path().join("b.txt")).unwrap();
    invalid(blame(&h.repo, "b.txt", None));
    let e = blame(&h.repo, "b.txt", Some("nope")).unwrap_err();
    assert!(matches!(e, EngineError::UnknownRev(_)), "{e}");
    // A branch without commits.
    let empty = Repo::new();
    empty.write("x.txt", "x\n");
    empty.stage(&["x.txt"]);
    invalid(blame(&empty, "x.txt", None));
}

#[test]
fn blame_of_a_binary_file_is_refused() {
    let repo = Repo::new();
    std::fs::write(repo.path().join("bin.dat"), b"x\0y\nz\n").unwrap();
    repo.stage(&["bin.dat"]);
    repo.commit("bin");
    for rev in [None, Some("HEAD")] {
        let e = blame(&repo, "bin.dat", rev).unwrap_err();
        assert!(matches!(e, EngineError::NotAllowed(_)), "{e}");
    }
    // An empty file is text, without lines.
    repo.commit_file("empty.txt", "", "empty");
    assert!(
        blame(&repo, "empty.txt", None)
            .unwrap()
            .blame
            .lines
            .is_empty()
    );
}

#[test]
fn blame_of_special_and_non_utf8_names() {
    let repo = repo_with_special_names();
    for name in ["-dash.txt", ":(top)magic.txt", "file*.txt"] {
        let b = blame(&repo, name, None).unwrap();
        assert_eq!(
            lines(&b),
            [line(&format!("after: {name}"), None, 1)],
            "{name}"
        );
        let b = blame(&repo, name, Some("HEAD")).unwrap();
        assert_eq!(b.blame.lines.len(), 1, "{name}");
        assert_eq!(b.blame.commits[0].path, RepoPath::from(name));
        assert_eq!(log(&repo, name, true).len(), 1, "{name}");
        assert_eq!(log(&repo, name, false).len(), 1, "{name}");
    }
    let fc = repo_with_file_changes();
    let name = RepoPath::from(NON_UTF8_NAME);
    let b = blame(&fc.repo, name.clone(), Some("HEAD")).unwrap();
    assert_eq!(lines(&b), [line("latin", Some(fc.changes), 1)]);
    assert_eq!(b.blame.commits[0].path, name);
    let entries = log(&fc.repo, name.clone(), true);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, name);
    assert_eq!(entries[0].status, Change::Added);
}

#[test]
fn blame_ignores_the_user_configuration() {
    let h = repo_with_renamed_file();
    std::fs::write(
        h.repo.path().join(".git-blame-ignore-revs"),
        format!("{}\n{}\n", h.one, h.two),
    )
    .unwrap();
    h.repo
        .git(&["config", "blame.ignoreRevsFile", ".git-blame-ignore-revs"]);
    h.repo.git(&["config", "blame.markIgnoredLines", "true"]);
    h.repo.git(&["config", "blame.markUnblamableLines", "true"]);
    h.repo.git(&["config", "blame.showRoot", "true"]);
    // Plain git skips the commits; the backend does not.
    let plain = h.repo.git(&["blame", "-s", "--", "b.txt"]);
    assert!(!plain.contains(&h.one.to_string()[..7]), "{plain}");
    let b = blame(&h.repo, "b.txt", Some("HEAD")).unwrap();
    assert_eq!(lines(&b)[0], line("ONE", Some(h.one), 1));
    assert_eq!(lines(&b)[1], line("TWO", Some(h.two), 2));
    let add = b.blame.commits.iter().find(|c| c.id == h.add).unwrap();
    assert!(add.boundary);
    let b = blame(&h.repo, "b.txt", None).unwrap();
    assert_eq!(lines(&b)[0], line("ONE", Some(h.one), 1));
}

#[test]
fn log_with_and_without_follow() {
    let h = repo_with_renamed_file();
    let b = |s: &str| s.to_owned();
    assert_eq!(
        summary(&log(&h.repo, "b.txt", true)),
        [
            (h.merge, b("b.txt"), None, Change::Modified),
            (h.one, b("b.txt"), None, Change::Modified),
            (h.five, b("b.txt"), None, Change::Modified),
            (h.four, b("b.txt"), None, Change::Modified),
            (h.rename, b("b.txt"), Some(b("a.txt")), Change::Renamed),
            (h.two, b("a.txt"), None, Change::Modified),
            (h.add, b("a.txt"), None, Change::Added),
        ]
    );
    let entries = log(&h.repo, "b.txt", false);
    assert_eq!(entries.len(), 5);
    assert_eq!(
        summary(&entries)[4],
        (h.rename, b("b.txt"), None, Change::Added)
    );
    let entries = log(&h.repo, "a.txt", false);
    assert_eq!(
        summary(&entries),
        [
            (h.rename, b("a.txt"), None, Change::Deleted),
            (h.two, b("a.txt"), None, Change::Modified),
            (h.add, b("a.txt"), None, Change::Added),
        ]
    );
    // The configuration does not change the result.
    h.repo.git(&["config", "log.follow", "true"]);
    h.repo.git(&["config", "diff.renames", "copies"]);
    h.repo.git(&["config", "log.showRoot", "false"]);
    assert_eq!(log(&h.repo, "b.txt", false).len(), 5);
    assert_eq!(log(&h.repo, "b.txt", true).len(), 7);
    assert!(log(&h.repo, "nope.txt", true).is_empty());
}

#[test]
fn log_pages_from_a_rev() {
    let h = repo_with_renamed_file();
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(Repo::opener()));
    let mut run = |params: serde_json::Value| registry.run("file.log", &mut session, params);
    // Without a repo.
    let e = run(json!({ "path": "b.txt" })).unwrap_err();
    assert!(matches!(e, EngineError::NoRepo), "{e}");
    registry
        .run(
            "repo.open",
            &mut session,
            json!({ "path": h.repo.path_str() }),
        )
        .unwrap();
    let mut run = |params: serde_json::Value| registry.run("file.log", &mut session, params);
    for follow in [true, false] {
        let all = run(json!({ "path": "b.txt", "follow": follow })).unwrap();
        let all = all["commits"].as_array().unwrap().clone();
        let mut paged = Vec::new();
        for skip in (0..all.len() + 2).step_by(2) {
            let page = run(json!({ "path": "b.txt", "follow": follow, "skip": skip, "limit": 2 }))
                .unwrap();
            assert_eq!(page["has_more"], skip + 2 < all.len(), "{follow} {skip}");
            paged.extend(page["commits"].as_array().unwrap().iter().cloned());
        }
        assert_eq!(paged, all, "follow {follow}");
    }
    let side = run(json!({ "path": "b.txt", "rev": "side" })).unwrap();
    let ids: Vec<&str> = side["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["commit"]["id"].as_str().unwrap())
        .collect();
    let want = [h.five, h.four, h.rename, h.two, h.add].map(|id| id.to_string());
    assert_eq!(ids, want);
    let e = run(json!({ "path": "b.txt", "rev": "nope" })).unwrap_err();
    assert!(matches!(e, EngineError::UnknownRev(_)), "{e}");
    let out = registry
        .run(
            "file.blame",
            &mut session,
            json!({ "path": "b.txt", "rev": "side" }),
        )
        .unwrap();
    assert_eq!(out["rev"], json!(h.five.to_string()));
    assert_eq!(out["lines"][4]["text"], "five");
    assert_eq!(out["path"], "b.txt");
}

#[test]
fn empty_repo_and_cancellation() {
    let empty = Repo::new();
    assert!(log(&empty, "x.txt", true).is_empty());
    let h = repo_with_renamed_file();
    let token = CancelToken::new();
    token.cancel();
    let job = JobContext::new().with_cancel(token);
    let backend = h.repo.open();
    let query = BlameQuery {
        path: RepoPath::from("b.txt"),
        rev: None,
    };
    let e = backend.blame(&query, &job).unwrap_err();
    assert!(matches!(e, EngineError::Cancelled { .. }), "{e}");
    let query = FileLogQuery {
        path: RepoPath::from("b.txt"),
        rev: None,
        skip: 0,
        limit: None,
        follow: true,
    };
    let e = backend.file_log(&query, &job).unwrap_err();
    assert!(matches!(e, EngineError::Cancelled { .. }), "{e}");
}
