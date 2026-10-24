//! Integration tests of `CliBackend` on the scenarios of `common`.

mod common;

use common::{
    History, OFFSET, Repo, repo_detached_head, repo_with_all_refs, repo_with_changes,
    repo_with_divergent_branches, repo_with_file_changes, repo_with_history,
    repo_with_merge_conflict,
};
use project_domain::{Change, Commit, CommitDetails, EntryKind, Head, Oid, Operation, RefKind};
use project_engine::{EngineError, GitBackend, LogQuery, Registry, RepoOpener, Session};
use project_git::{CliOpener, GitVersion};
use serde_json::json;
use std::sync::Arc;

fn ids(commits: &[Commit]) -> Vec<Oid> {
    commits.iter().map(|c| c.id).collect()
}

#[test]
fn scenarios_are_deterministic() {
    let first = repo_with_history();
    let second = repo_with_history();
    let all = |h: &History| [h.c1, h.c2, h.f1, h.f2, h.m];
    assert_eq!(all(&first), all(&second));
    // The same id across runs too: it depends only on content, author and dates.
    assert_eq!(
        first.c1.to_string(),
        "3c21eb807c30d38a30b2dd63545cd2a7f21891bf"
    );
}

#[test]
fn git_version_is_supported() {
    let version = Repo::opener().git_version().unwrap();
    assert!(version >= GitVersion::MIN, "{version}");
}

#[test]
fn missing_git_is_a_clear_error() {
    let repo = Repo::new();
    let err = CliOpener::with_program("/no/git/here")
        .open(repo.path_str())
        .err()
        .unwrap();
    assert!(matches!(err, EngineError::GitNotFound(_)), "{err}");
}

#[cfg(unix)]
#[test]
fn old_git_is_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("git");
    std::fs::write(&fake, "#!/bin/sh\necho 'git version 2.20.1'\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let repo = Repo::new();
    let err = CliOpener::with_program(&fake)
        .open(repo.path_str())
        .err()
        .unwrap();
    let EngineError::GitTooOld { found, required } = err else {
        panic!("{err}")
    };
    assert_eq!((found.as_str(), required.as_str()), ("2.20.1", "2.30.0"));
}

#[test]
fn opening_a_plain_folder_fails() {
    let dir = tempfile::tempdir().unwrap();
    // Without this git would climb up to any repo that contains the temporary directory.
    let opener = Repo::opener().env("GIT_CEILING_DIRECTORIES", dir.path().parent().unwrap());
    let err = opener.open(dir.path().to_str().unwrap()).err().unwrap();
    let EngineError::RepoNotFound { reason, .. } = err else {
        panic!("{err}")
    };
    assert!(reason.contains("not a git repository"), "{reason}");

    let missing = dir.path().join("does-not-exist");
    let err = opener.open(missing.to_str().unwrap()).err().unwrap();
    assert!(matches!(err, EngineError::RepoNotFound { .. }), "{err}");

    let err = opener.open("").err().unwrap();
    assert!(matches!(err, EngineError::RepoNotFound { .. }), "{err}");
}

#[test]
fn opening_a_bare_repo_fails() {
    let bare = Repo::bare();
    let err = Repo::opener().open(bare.path_str()).err().unwrap();
    let EngineError::RepoNotFound { reason, .. } = err else {
        panic!("{err}")
    };
    assert!(reason.contains("bare"), "{reason}");
}

#[test]
fn opening_a_subfolder_uses_the_worktree_root() {
    let repo = Repo::new();
    repo.commit_file("src/main.rs", "fn main() {}\n", "first");
    let backend = Repo::opener()
        .open_cli(repo.path().join("src").to_str().unwrap())
        .unwrap();
    assert_eq!(
        backend.workdir().canonicalize().unwrap(),
        repo.path().canonicalize().unwrap()
    );
    assert_eq!(backend.git_dir().file_name().unwrap(), ".git");
}

#[test]
fn empty_repo() {
    let repo = Repo::new();
    let backend = repo.open();
    let head = Head::Branch {
        name: "main".to_owned(),
        target: None,
    };
    assert_eq!(backend.head().unwrap(), head);
    assert!(backend.refs().unwrap().is_empty());
    assert!(backend.log(&LogQuery::all()).unwrap().is_empty());
    let status = backend.status().unwrap();
    assert_eq!(status.head, head);
    assert!(status.is_clean());
    assert_eq!(status.operation, None);
}

#[test]
fn log_reads_the_whole_history() {
    let h = repo_with_history();
    let log = h.repo.open().log(&LogQuery::all()).unwrap();
    // Children before parents, then the most recent by committer date.
    assert_eq!(ids(&log), [h.m, h.f2, h.f1, h.c2, h.c1]);

    let merge = &log[0];
    assert!(merge.is_merge());
    assert_eq!(merge.parents, [h.c2, h.f2]);
    assert_eq!(merge.summary, "Merge branch 'feature'");

    let c2 = &log[3];
    assert_eq!(c2.summary, "second on main");
    assert_eq!(c2.body, "Body\non two lines.");
    assert_eq!(c2.author.name, "Ada");
    assert_eq!(c2.author.email, "ada@example.com");
    assert_eq!(c2.committer.name, "Bob");
    let expected = h
        .repo
        .git(&["log", "-1", "--format=%at %ai", &h.c2.to_string()]);
    assert_eq!(
        expected.split(' ').next(),
        Some(c2.author.time.to_string().as_str())
    );
    assert!(expected.ends_with(OFFSET), "{expected}");
    assert_eq!(c2.author.offset_min, 120);

    let root = &log[4];
    assert!(root.parents.is_empty());
    assert_eq!(root.body, "");
}

#[test]
fn log_pages_and_revs() {
    let h = repo_with_history();
    let backend = h.repo.open();
    let page = backend.log(&LogQuery::page(1, 2)).unwrap();
    assert_eq!(ids(&page), [h.f2, h.f1]);
    assert!(backend.log(&LogQuery::page(0, 0)).unwrap().is_empty());
    assert!(backend.log(&LogQuery::page(10, 5)).unwrap().is_empty());

    let feature = backend.log(&LogQuery::all().from_rev("feature")).unwrap();
    assert_eq!(ids(&feature), [h.f2, h.f1, h.c1]);
    let short = h.c2.to_string()[..8].to_owned();
    let from_id = backend.log(&LogQuery::page(0, 1).from_rev(short)).unwrap();
    assert_eq!(ids(&from_id), [h.c2]);
    let full = backend
        .log(&LogQuery::all().from_rev("refs/heads/feature"))
        .unwrap();
    assert_eq!(full.len(), 3);
}

#[test]
fn log_rejects_unknown_revs_without_running_options() {
    let h = repo_with_history();
    let backend = h.repo.open();
    for rev in [
        "nobody",
        "--all",
        "-n1",
        "--output=/tmp/x",
        "",
        "main..feature",
    ] {
        let err = backend.log(&LogQuery::all().from_rev(rev)).unwrap_err();
        assert!(
            matches!(err, EngineError::UnknownRev(ref r) if r == rev),
            "{rev}: {err}"
        );
    }
}

#[test]
fn head_on_branch() {
    let h = repo_with_history();
    assert_eq!(
        h.repo.open().head().unwrap(),
        Head::Branch {
            name: "main".to_owned(),
            target: Some(h.m),
        }
    );
}

#[test]
fn head_detached() {
    let h = repo_detached_head();
    let backend = h.repo.open();
    assert_eq!(backend.head().unwrap(), Head::Detached(h.f1));
    assert_eq!(ids(&backend.log(&LogQuery::all()).unwrap()), [h.f1, h.c1]);
    let status = backend.status().unwrap();
    assert_eq!(status.head, Head::Detached(h.f1));
    assert_eq!(status.upstream, None);
}

#[test]
fn refs_with_tags_stash_and_upstream() {
    let (h, _origin) = repo_with_all_refs();
    let repo = &h.repo;
    let backend = repo.open();
    let refs = backend.refs().unwrap();
    let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "refs/heads/feature",
            "refs/heads/main",
            "refs/remotes/origin/main",
            "refs/stash",
            "refs/tags/light",
            "refs/tags/v1",
        ]
    );
    let by_name = |name: &str| refs.iter().find(|r| r.name == name).unwrap();

    let main = by_name("refs/heads/main");
    assert_eq!(main.kind, RefKind::LocalBranch);
    assert_eq!(main.target, h.m);
    let upstream = main.upstream.as_ref().unwrap();
    assert_eq!(upstream.name, "refs/remotes/origin/main");
    assert_eq!((upstream.ahead, upstream.behind), (Some(3), Some(0)));
    assert_eq!(by_name("refs/heads/feature").upstream, None);

    let remote = by_name("refs/remotes/origin/main");
    assert_eq!((remote.kind, remote.target), (RefKind::RemoteBranch, h.c2));
    let stash = by_name("refs/stash");
    assert_eq!(
        (stash.kind, stash.target),
        (RefKind::Stash, repo.rev("refs/stash"))
    );

    let light = by_name("refs/tags/light");
    assert_eq!(
        (light.kind, light.target, light.tag_object),
        (RefKind::Tag, h.c1, None)
    );
    let annotated = by_name("refs/tags/v1");
    assert_eq!(annotated.target, h.c2);
    assert_eq!(annotated.tag_object, Some(repo.rev("refs/tags/v1")));

    let status = backend.status().unwrap();
    assert!(status.is_clean());
    let upstream = status.upstream.unwrap();
    assert_eq!(upstream.name, "origin/main");
    assert_eq!((upstream.ahead, upstream.behind), (Some(3), Some(0)));

    // The annotated tag is also usable as a log rev.
    let from_tag = backend.log(&LogQuery::all().from_rev("v1")).unwrap();
    assert_eq!(ids(&from_tag), [h.c2, h.c1]);
}

#[test]
fn status_lists_changes() {
    let repo = repo_with_changes();
    let status = repo.open().status().unwrap();
    assert!(!status.is_clean());
    let entry = |path: &str| {
        status
            .entries
            .iter()
            .find(|e| e.path.to_str() == Some(path))
            .unwrap_or_else(|| panic!("{path} not in {:?}", status.entries))
    };
    let a = entry("a.txt");
    assert_eq!(
        (a.index, a.worktree),
        (Change::Unmodified, Change::Modified)
    );
    assert!(a.is_unstaged() && !a.is_staged());
    assert_eq!(entry("new.txt").index, Change::Added);
    let renamed = entry("renamed.txt");
    assert_eq!(renamed.index, Change::Renamed);
    assert_eq!(
        renamed.original_path().and_then(|p| p.to_str()),
        Some("old.txt")
    );
    // With `--untracked-files=all` the file, not just the directory.
    let untracked = entry("dir/untracked file.txt");
    assert!(untracked.is_untracked());
    assert!(matches!(untracked.kind, EntryKind::Untracked));
    assert_eq!(status.entries.len(), 4);
}

#[test]
fn status_reports_merge_conflict() {
    let repo = repo_with_merge_conflict();
    let backend = repo.open();
    let status = backend.status().unwrap();
    assert_eq!(status.operation, Some(Operation::Merge));
    assert!(status.has_conflicts());
    assert!(status.entries[0].is_conflict());

    repo.git(&["merge", "--abort"]);
    let status = backend.status().unwrap();
    assert_eq!(status.operation, None);
    assert!(status.is_clean());
}

#[test]
fn status_reports_cherry_pick_and_bisect() {
    let (repo, theirs) = repo_with_divergent_branches();
    let backend = repo.open();

    repo.git_fails(&["cherry-pick", &theirs.to_string()]);
    assert_eq!(
        backend.status().unwrap().operation,
        Some(Operation::CherryPick)
    );
    repo.git(&["cherry-pick", "--abort"]);

    repo.git(&["bisect", "start"]);
    assert_eq!(backend.status().unwrap().operation, Some(Operation::Bisect));
    repo.git(&["bisect", "reset"]);
    assert_eq!(backend.status().unwrap().operation, None);
}

#[test]
fn confine_to_stops_the_search_at_the_root() {
    let outer = Repo::new();
    outer.commit_file("a.txt", "a\n", "outside the root");
    let root = outer.path().join("root");
    std::fs::create_dir_all(root.join("dir")).unwrap();
    let root = root.canonicalize().unwrap();
    let plain = root.join("dir");

    // Without a boundary git climbs up to the repo that contains the root.
    let free = Repo::opener().open_cli(plain.to_str().unwrap()).unwrap();
    assert_eq!(
        free.workdir().canonicalize().unwrap(),
        outer.path().canonicalize().unwrap()
    );

    let confined = Repo::opener().confine_to(&root);
    for dir in [&root, &plain] {
        let err = confined.open(dir.to_str().unwrap()).err().unwrap();
        assert!(matches!(err, EngineError::RepoNotFound { .. }), "{err}");
    }

    // A repo inside the root can be opened.
    let inner = root.join("inner");
    let status = std::process::Command::new("git")
        .args(["-c", "init.defaultBranch=main", "init", "--quiet"])
        .arg(&inner)
        .envs(common::GIT_ENV.iter().copied())
        .status()
        .unwrap();
    assert!(status.success());
    let backend = confined.open_cli(inner.to_str().unwrap()).unwrap();
    assert_eq!(backend.workdir().canonicalize().unwrap(), inner);
}

/// `(status, origin, path)` of the changed files, for compact comparisons.
fn changes(details: &CommitDetails) -> Vec<(Change, Option<String>, String)> {
    details
        .files
        .iter()
        .map(|f| {
            (
                f.change,
                f.original.as_ref().map(ToString::to_string),
                f.path.to_string(),
            )
        })
        .collect()
}

#[test]
fn show_commit_root_modify_and_merge() {
    let h = repo_with_history();
    let backend = h.repo.open();

    let root = backend.show_commit(&h.c1.to_string()).unwrap();
    assert_eq!(root.commit, backend.log(&LogQuery::all()).unwrap()[4]);
    assert_eq!(root.compared_to, None);
    assert_eq!(changes(&root), [(Change::Added, None, "a.txt".to_owned())]);
    let a = &root.files[0];
    assert_eq!((a.old_mode, a.new_mode), (0, 0o100644));
    assert_eq!(a.old_oid, Oid::from_bytes([0; Oid::LEN]));
    assert_eq!(a.new_oid, h.repo.rev(&format!("{}:a.txt", h.c1)));

    let c2 = backend.show_commit(&h.c2.to_string()).unwrap();
    assert_eq!(c2.commit.body, "Body\non two lines.");
    assert_eq!(c2.compared_to, Some(h.c1));
    assert_eq!(changes(&c2), [(Change::Modified, None, "a.txt".to_owned())]);
    assert_eq!(c2.files[0].old_oid, a.new_oid);

    // The merge is compared with the first parent (`c2`): it brings `b.txt` from `feature`.
    let merge = backend.show_commit("HEAD").unwrap();
    assert_eq!(merge.commit.id, h.m);
    assert_eq!(merge.commit.parents, [h.c2, h.f2]);
    assert_eq!(merge.compared_to, Some(h.c2));
    assert_eq!(changes(&merge), [(Change::Added, None, "b.txt".to_owned())]);
    assert_eq!(
        merge.files[0].new_oid,
        h.repo.rev(&format!("{}:b.txt", h.f2))
    );

    // Abbreviated id, branch name and full name lead to the same commit.
    let short = h.f2.to_string()[..7].to_owned();
    for rev in [short.as_str(), "feature", "refs/heads/feature"] {
        assert_eq!(backend.show_commit(rev).unwrap().commit.id, h.f2, "{rev}");
    }
}

#[test]
fn show_commit_every_kind_of_change() {
    let fc = repo_with_file_changes();
    let backend = fc.repo.open();
    let details = backend.show_commit(&fc.changes.to_string()).unwrap();
    assert_eq!(details.compared_to, Some(fc.base));
    let s = |v: &str| v.to_owned();
    let mut expected = vec![
        (Change::Deleted, None, s("deleted.txt")),
        (Change::Added, None, s("dir/with space.txt")),
        (Change::Renamed, Some(s("move.txt")), s("moved.txt")),
        (Change::Renamed, Some(s("old.txt")), s("new.txt")),
        (Change::Modified, None, s("script.sh")),
    ];
    if cfg!(unix) {
        expected.insert(0, (Change::Added, None, s("cafe\u{fffd}.txt")));
    }
    assert_eq!(changes(&details), expected);

    let file = |name: &str| {
        details
            .files
            .iter()
            .find(|f| f.path.to_str() == Some(name))
            .unwrap()
    };
    let exact = file("new.txt");
    assert_eq!(
        (exact.score, exact.old_oid == exact.new_oid),
        (Some(100), true)
    );
    let moved = file("moved.txt");
    assert!(moved.score.unwrap() < 100, "{:?}", moved.score);
    assert_ne!(moved.old_oid, moved.new_oid);
    let script = file("script.sh");
    assert_eq!((script.old_mode, script.new_mode), (0o100644, 0o100755));
    let deleted = file("deleted.txt");
    assert_eq!(
        (deleted.new_mode, deleted.new_oid),
        (0, Oid::from_bytes([0; Oid::LEN]))
    );
    // The non-UTF-8 name keeps its exact bytes.
    #[cfg(unix)]
    assert!(
        details
            .files
            .iter()
            .any(|f| f.path.as_bytes() == common::NON_UTF8_NAME)
    );

    let empty = backend.show_commit(&fc.empty.to_string()).unwrap();
    assert_eq!(empty.commit.summary, "empty");
    assert_eq!(empty.compared_to, Some(fc.changes));
    assert!(empty.files.is_empty());
}

#[test]
fn show_commit_follows_tags_and_rejects_bad_revs() {
    let (h, _origin) = repo_with_all_refs();
    let backend = h.repo.open();
    // An annotated tag leads to the commit, not to the tag object.
    assert_eq!(backend.show_commit("v1").unwrap().commit.id, h.c2);
    assert_eq!(backend.show_commit("light").unwrap().commit.id, h.c1);

    let tree = h.repo.rev("HEAD^{tree}").to_string();
    for rev in [
        "nobody",
        "--all",
        "-n1",
        "--output=/tmp/x",
        "",
        "main..feature",
    ] {
        let err = backend.show_commit(rev).unwrap_err();
        assert!(
            matches!(err, EngineError::UnknownRev(ref r) if r == rev),
            "{rev}: {err}"
        );
    }

    // An object that is not a commit: git error with the reason.
    let err = backend.show_commit(&tree).unwrap_err();
    assert!(
        matches!(&err, EngineError::Git { stderr, .. } if stderr.contains("tree")),
        "{err}"
    );

    let empty = Repo::new();
    assert!(matches!(
        empty.open().show_commit("HEAD"),
        Err(EngineError::UnknownRev(_))
    ));
}

#[test]
fn commit_show_command_on_a_real_repo() {
    let fc = repo_with_file_changes();
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(Repo::opener()));
    let err = registry
        .run("commit.show", &mut session, json!({ "id": "HEAD" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NoRepo), "{err}");

    registry
        .run(
            "repo.open",
            &mut session,
            json!({ "path": fc.repo.path_str() }),
        )
        .unwrap();
    let out = registry
        .run(
            "commit.show",
            &mut session,
            json!({ "id": fc.changes.to_string() }),
        )
        .unwrap();
    assert_eq!(out["commit"]["id"], fc.changes.to_string());
    assert_eq!(out["commit"]["summary"], "every kind of change");
    assert_eq!(out["compared_to"], fc.base.to_string());
    let files = out["files"].as_array().unwrap();
    let renamed = files.iter().find(|f| f["path"] == "new.txt").unwrap();
    assert_eq!(renamed["change"], "Renamed");
    assert_eq!(renamed["original"], "old.txt");
    assert_eq!(renamed["score"], 100);
    // In JSON a non-UTF-8 path is a byte array.
    #[cfg(unix)]
    assert!(
        files
            .iter()
            .any(|f| f["path"] == json!(common::NON_UTF8_NAME))
    );
    let details: CommitDetails = serde_json::from_value(out).unwrap();
    assert_eq!(
        details,
        fc.repo.open().show_commit(&fc.changes.to_string()).unwrap()
    );

    let err = registry
        .run("commit.show", &mut session, json!({ "id": "--all" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
}

/// `repo.open` returns the absolute worktree root, also when opened from a subfolder.
#[test]
fn open_returns_the_worktree_root() {
    let h = repo_with_history();
    let sub = h.repo.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let root = h.repo.path().canonicalize().unwrap();
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    let out = r
        .run(
            "repo.open",
            &mut s,
            json!({ "path": sub.to_str().unwrap() }),
        )
        .unwrap();
    let path = std::path::Path::new(out["path"].as_str().unwrap());
    assert!(path.is_absolute(), "{path:?}");
    assert_eq!(path.canonicalize().unwrap(), root);
}
