//! Test di integrazione di `CliBackend` sugli scenari di `common`.

mod common;

use common::{
    History, OFFSET, Repo, repo_detached_head, repo_with_all_refs, repo_with_changes,
    repo_with_divergent_branches, repo_with_history, repo_with_merge_conflict,
};
use project_domain::{Change, Commit, EntryKind, Head, Oid, Operation, RefKind};
use project_engine::{EngineError, GitBackend, LogQuery, RepoOpener};
use project_git::{CliOpener, GitVersion};

fn ids(commits: &[Commit]) -> Vec<Oid> {
    commits.iter().map(|c| c.id).collect()
}

#[test]
fn scenarios_are_deterministic() {
    let first = repo_with_history();
    let second = repo_with_history();
    let all = |h: &History| [h.c1, h.c2, h.f1, h.f2, h.m];
    assert_eq!(all(&first), all(&second));
    // Lo stesso id anche tra un'esecuzione e l'altra: dipende solo da contenuto, autore e date.
    assert_eq!(
        first.c1.to_string(),
        "d73fe48854a640cf507c31d41c382df71580568e"
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
    let err = CliOpener::with_program("/nessun/git/qui")
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
    // Senza questo git risalirebbe fino a un eventuale repo che contiene la cartella temporanea.
    let opener = Repo::opener().env("GIT_CEILING_DIRECTORIES", dir.path().parent().unwrap());
    let err = opener.open(dir.path().to_str().unwrap()).err().unwrap();
    let EngineError::RepoNotFound { reason, .. } = err else {
        panic!("{err}")
    };
    assert!(reason.contains("not a git repository"), "{reason}");

    let missing = dir.path().join("non-esiste");
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
    repo.commit_file("src/main.rs", "fn main() {}\n", "primo");
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
    // Figli prima dei genitori, poi il più recente per data del committer.
    assert_eq!(ids(&log), [h.m, h.f2, h.f1, h.c2, h.c1]);

    let merge = &log[0];
    assert!(merge.is_merge());
    assert_eq!(merge.parents, [h.c2, h.f2]);
    assert_eq!(merge.summary, "Merge branch 'feature'");

    let c2 = &log[3];
    assert_eq!(c2.summary, "secondo su main");
    assert_eq!(c2.body, "Corpo\nsu due righe.");
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
        "nessuno",
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
            "refs/tags/leggero",
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

    let light = by_name("refs/tags/leggero");
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

    // Il tag annotato si usa anche come rev del log.
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
            .unwrap_or_else(|| panic!("{path} non in {:?}", status.entries))
    };
    let a = entry("a.txt");
    assert_eq!(
        (a.index, a.worktree),
        (Change::Unmodified, Change::Modified)
    );
    assert!(a.is_unstaged() && !a.is_staged());
    assert_eq!(entry("nuovo.txt").index, Change::Added);
    let renamed = entry("rinominato.txt");
    assert_eq!(renamed.index, Change::Renamed);
    assert_eq!(
        renamed.original_path().and_then(|p| p.to_str()),
        Some("vecchio.txt")
    );
    // Con `--untracked-files=all` il file, non la sola cartella.
    let untracked = entry("cartella/non tracciato.txt");
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
