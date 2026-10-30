use super::{GitBackend, GitRead, LogQuery, MemoryBackend, MemoryOpener, RepoOpener};
use crate::EngineError;
use project_domain::{Change, ChangedFile, Commit, Head, Oid, RefKind, Upstream};
use std::collections::BTreeSet;

fn sample() -> MemoryBackend {
    MemoryBackend::sample()
}

fn log(repo: &MemoryBackend, query: LogQuery) -> Vec<Commit> {
    repo.log(&query).unwrap()
}

fn summaries(commits: &[Commit]) -> Vec<&str> {
    commits.iter().map(|c| c.summary.as_str()).collect()
}

fn ref_target(repo: &MemoryBackend, name: &str) -> Oid {
    repo.refs()
        .unwrap()
        .into_iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("missing {name}"))
        .target
}

#[test]
fn head_on_main_at_the_last_commit() {
    let repo = sample();
    let head = repo.head().unwrap();
    assert_eq!(
        head,
        Head::Branch {
            name: "main".into(),
            target: Some(ref_target(&repo, "refs/heads/main")),
        }
    );
    let first = &log(&repo, LogQuery::page(0, 1))[0];
    assert_eq!(head.target(), Some(first.id));
    assert_eq!(first.summary, "Commit panel");
}

#[test]
fn refs_in_name_order_with_kinds_and_upstream() {
    let repo = sample();
    let refs = repo.refs().unwrap();
    let names: Vec<_> = refs.iter().map(|r| (r.name.as_str(), r.kind)).collect();
    assert_eq!(
        names,
        [
            ("refs/heads/feature/graph", RefKind::LocalBranch),
            ("refs/heads/main", RefKind::LocalBranch),
            ("refs/remotes/origin/feature/graph", RefKind::RemoteBranch),
            ("refs/remotes/origin/main", RefKind::RemoteBranch),
            ("refs/tags/v0.1.0", RefKind::Tag),
        ]
    );
    let by_name = |n: &str| refs.iter().find(|r| r.name == n).unwrap();
    assert_eq!(
        by_name("refs/heads/main").upstream,
        Some(Upstream {
            name: "refs/remotes/origin/main".into(),
            ahead: Some(1),
            behind: Some(0),
        })
    );
    assert_eq!(
        by_name("refs/heads/feature/graph").upstream,
        Some(Upstream {
            name: "refs/remotes/origin/feature/graph".into(),
            ahead: Some(0),
            behind: Some(0),
        })
    );
    for r in &refs {
        if r.kind != RefKind::LocalBranch {
            assert_eq!(r.upstream, None, "{}", r.name);
        }
        if r.kind != RefKind::Tag {
            assert_eq!(r.tag_object, None, "{}", r.name);
        }
    }
    assert_eq!(
        by_name("refs/heads/feature/graph").target,
        by_name("refs/remotes/origin/feature/graph").target
    );
}

#[test]
fn the_annotated_tag_points_to_the_merge() {
    let repo = sample();
    let tag = repo
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.kind == RefKind::Tag)
        .unwrap();
    assert_eq!(tag.short_name(), "v0.1.0");
    let tag_object = tag.tag_object.expect("annotated tag");
    assert_ne!(tag_object, tag.target);
    let merge = log(&repo, LogQuery::default().from_rev("v0.1.0"));
    assert_eq!(merge[0].id, tag.target);
    assert!(merge[0].is_merge());
}

#[test]
fn full_log_children_before_parents() {
    let repo = sample();
    let all = log(&repo, LogQuery::all());
    assert_eq!(all.len(), 12);
    assert_eq!(
        summaries(&all),
        [
            "Commit panel",
            "Ref parser",
            "Merge branch 'feature/graph'",
            "Property tests for the lanes",
            "Status parser",
            "Lanes: reuse the free columns",
            "Fix a typo in the README",
            "Graph lanes: first version",
            "Log parser",
            "Project structure",
            "Add the README",
            "First commit",
        ]
    );
    let ids: BTreeSet<_> = all.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), all.len(), "no repeated commit");
    for (i, commit) in all.iter().enumerate() {
        for parent in &commit.parents {
            let pos = all.iter().position(|c| c.id == *parent).unwrap();
            assert!(pos > i, "{} before child {}", parent, commit.id);
        }
    }
    assert!(
        all.windows(2)
            .all(|w| w[0].committer.time >= w[1].committer.time)
    );
    assert_eq!(all.last().unwrap().parents, []);
    assert!(all.iter().any(|c| !c.body.is_empty()));
}

#[test]
fn pagination_with_skip_and_limit() {
    let repo = sample();
    let all = log(&repo, LogQuery::all());
    assert_eq!(log(&repo, LogQuery::page(3, 4)), all[3..7]);
    assert_eq!(log(&repo, LogQuery::page(10, 5)), all[10..]);
    assert_eq!(log(&repo, LogQuery::page(12, 5)), []);
    assert_eq!(log(&repo, LogQuery::page(100, 5)), []);
    assert_eq!(log(&repo, LogQuery::page(0, 0)), []);
    let skip_only = LogQuery {
        skip: 5,
        ..LogQuery::default()
    };
    assert_eq!(log(&repo, skip_only), all[5..]);
    // Consecutive pages rebuild the log.
    let pages: Vec<Commit> = (0..4)
        .flat_map(|p| log(&repo, LogQuery::page(p * 5, 5)))
        .collect();
    assert_eq!(pages, all);
}

#[test]
fn log_from_a_branch_does_not_include_main() {
    let repo = sample();
    let feature = log(&repo, LogQuery::default().from_rev("feature/graph"));
    assert_eq!(
        summaries(&feature),
        [
            "Property tests for the lanes",
            "Lanes: reuse the free columns",
            "Graph lanes: first version",
            "Log parser",
            "Project structure",
            "Add the README",
            "First commit",
        ]
    );
    let page = log(&repo, LogQuery::page(1, 2).from_rev("feature/graph"));
    assert_eq!(page, feature[1..3]);
}

#[test]
fn rev_by_full_short_name_head_and_id() {
    let repo = sample();
    let all = log(&repo, LogQuery::all());
    let first = |rev: &str| log(&repo, LogQuery::page(0, 1).from_rev(rev))[0].id;
    let main = ref_target(&repo, "refs/heads/main");
    assert_eq!(first("HEAD"), main);
    assert_eq!(first("main"), main);
    assert_eq!(first("refs/heads/main"), main);
    assert_eq!(first("heads/main"), main);
    assert_eq!(first("origin/main"), all[1].id);
    assert_eq!(first("refs/remotes/origin/main"), all[1].id);
    let target = all[6].id;
    assert_eq!(first(&target.to_string()), target);
    assert_eq!(first(&target.to_string().to_uppercase()), target);
    assert_eq!(first(&target.short()), target);
    // From a commit on main's side before the merge: no commits of feature/graph.
    assert_eq!(
        summaries(&log(&repo, LogQuery::default().from_rev(target.short()))),
        [
            "Fix a typo in the README",
            "Log parser",
            "Project structure",
            "Add the README",
            "First commit",
        ]
    );
}

#[test]
fn unknown_rev_is_an_error() {
    let repo = sample();
    for rev in ["missing", "origin/missing", "", "abc", "0000000", "HEAD~1"] {
        let err = repo.log(&LogQuery::default().from_rev(rev)).expect_err(rev);
        assert!(
            matches!(&err, EngineError::UnknownRev(r) if r == rev),
            "{err}"
        );
    }
}

#[test]
fn the_merge_has_two_parents() {
    let repo = sample();
    let all = log(&repo, LogQuery::all());
    let merges: Vec<_> = all.iter().filter(|c| c.is_merge()).collect();
    assert_eq!(merges.len(), 1);
    let merge = merges[0];
    assert_eq!(merge.parents.len(), 2);
    let summary_of = |id: &Oid| all.iter().find(|c| c.id == *id).unwrap().summary.as_str();
    // First parent: main's side; second: the tip of the merged branch.
    assert_eq!(summary_of(&merge.parents[0]), "Status parser");
    assert_eq!(
        merge.parents[1],
        ref_target(&repo, "refs/heads/feature/graph")
    );
}

#[test]
fn dirty_status_with_short_upstream() {
    let repo = sample();
    let status = repo.status().unwrap();
    assert_eq!(status.head, repo.head().unwrap());
    assert_eq!(
        status.upstream,
        Some(Upstream {
            name: "origin/main".into(),
            ahead: Some(1),
            behind: Some(0),
        })
    );
    assert_eq!(status.operation, None);
    assert!(!status.is_clean());
    assert!(!status.has_conflicts());
    let codes: Vec<_> = status
        .entries
        .iter()
        .map(|e| {
            (
                e.path.to_string(),
                format!("{}{}", e.index.letter(), e.worktree.letter()),
            )
        })
        .collect();
    assert_eq!(
        codes,
        [
            ("README.md".to_owned(), "MM".to_owned()),
            ("docs/guide.md".to_owned(), "A.".to_owned()),
            ("src/lib.rs".to_owned(), ".M".to_owned()),
            ("note.txt".to_owned(), "??".to_owned()),
        ]
    );
    let count = |f: fn(&project_domain::StatusEntry) -> bool| {
        status.entries.iter().filter(|e| f(e)).count()
    };
    assert_eq!(count(|e| e.is_staged()), 2);
    assert_eq!(count(|e| e.is_unstaged()), 2);
    assert_eq!(count(|e| e.is_untracked()), 1);
    assert_eq!(status.entries[1].index, Change::Added);
}

#[test]
fn empty_repo() {
    let repo = MemoryBackend::new();
    assert_eq!(
        repo.head().unwrap(),
        Head::Branch {
            name: "main".into(),
            target: None
        }
    );
    assert_eq!(repo.refs().unwrap(), []);
    assert_eq!(log(&repo, LogQuery::all()), []);
    assert!(matches!(
        repo.log(&LogQuery::default().from_rev("HEAD")),
        Err(EngineError::UnknownRev(_))
    ));
    let status = repo.status().unwrap();
    assert!(status.is_clean());
    assert_eq!(status.upstream, None);
}

#[test]
fn detached_head_and_gone_upstream() {
    let mut repo = sample();
    let target = log(&repo, LogQuery::all())[4].id;
    repo.set_branch("old", target);
    repo.set_upstream("old", "origin/old");
    let old = repo
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.short_name() == "old")
        .unwrap();
    assert_eq!(
        old.upstream,
        Some(Upstream {
            name: "refs/remotes/origin/old".into(),
            ahead: None,
            behind: None,
        })
    );

    repo.detach_head(target);
    assert_eq!(repo.head().unwrap(), Head::Detached(target));
    assert_eq!(repo.status().unwrap().upstream, None);
    assert_eq!(log(&repo, LogQuery::page(0, 1))[0].id, target);
}

#[test]
fn ahead_and_behind_from_the_graph() {
    let mut repo = sample();
    // origin/main moves ahead by a commit that main does not have: 1 ahead, 1 behind.
    let base = ref_target(&repo, "refs/remotes/origin/main");
    let mut extra = log(&repo, LogQuery::page(0, 1))[0].clone();
    extra.id = MemoryBackend::fake_oid(99);
    extra.parents = vec![base];
    extra.summary = "Commit only on the remote".into();
    repo.insert_commit(extra.clone());
    repo.set_remote_branch("origin/main", extra.id);
    let status = repo.status().unwrap();
    let upstream = status.upstream.unwrap();
    assert_eq!((upstream.ahead, upstream.behind), (Some(1), Some(1)));
}

#[test]
fn fake_ids_deterministic_and_distinct() {
    assert_eq!(MemoryBackend::fake_oid(7), MemoryBackend::fake_oid(7));
    let ids: BTreeSet<_> = (0..10_000).map(MemoryBackend::fake_oid).collect();
    assert_eq!(ids.len(), 10_000);
    assert!(!ids.contains(&Oid::from_bytes([0; Oid::LEN])));
    // The sample is the same on every construction.
    assert_eq!(MemoryBackend::sample(), MemoryBackend::sample());
}

#[test]
fn in_memory_opener() {
    let opener = MemoryOpener::sample();
    let repo: Box<dyn GitBackend> = opener.open(MemoryOpener::SAMPLE_PATH).unwrap();
    assert_eq!(repo.log(&LogQuery::all()).unwrap().len(), 12);
    let err = opener.open("/other").err().unwrap();
    assert!(
        matches!(&err, EngineError::RepoNotFound { path, .. } if path == "/other"),
        "{err}"
    );
    assert!(err.to_string().contains(MemoryOpener::SAMPLE_PATH), "{err}");

    let mut opener = MemoryOpener::new();
    assert!(opener.open("empty").is_err());
    opener.insert("empty", MemoryBackend::new());
    assert_eq!(opener.paths(), ["empty"]);
    assert!(opener.open("empty").unwrap().refs().unwrap().is_empty());
}

#[test]
fn trait_object_send_and_sync() {
    fn send<T: Send + ?Sized>() {}
    fn send_sync<T: Send + Sync + ?Sized>() {}
    send::<dyn GitBackend>();
    send::<Box<dyn GitBackend>>();
    send_sync::<dyn RepoOpener>();
    send_sync::<MemoryOpener>();
}

#[test]
fn log_query_from_json() {
    let q: LogQuery = serde_json::from_str("{}").unwrap();
    assert_eq!(q, LogQuery::all());
    let q: LogQuery = serde_json::from_str(r#"{"skip":2,"limit":3,"rev":"main"}"#).unwrap();
    assert_eq!(q, LogQuery::page(2, 3).from_rev("main"));
    let q: LogQuery = serde_json::from_str(r#"{"limit":null}"#).unwrap();
    assert_eq!(q.limit, None);
    assert!(serde_json::from_str::<LogQuery>(r#"{"count":1}"#).is_err());
    assert!(serde_json::from_str::<LogQuery>(r#"{"skip":-1}"#).is_err());
    let q: LogQuery = serde_json::from_str(r#"{"all":true,"limit":5}"#).unwrap();
    assert_eq!(q, LogQuery::page(0, 5).from_all_refs());
    assert!(serde_json::from_str::<LogQuery>(r#"{"all":1}"#).is_err());
}

/// A new commit `n` child of `parent`, with a date after all those of the sample.
fn extra_commit(repo: &mut MemoryBackend, n: u64, parent: Oid, summary: &str) -> Oid {
    let mut commit = log(repo, LogQuery::page(0, 1))[0].clone();
    commit.id = MemoryBackend::fake_oid(n);
    commit.parents = vec![parent];
    commit.summary = summary.into();
    commit.committer.time += i64::try_from(n).unwrap() * 60;
    repo.insert_commit(commit);
    MemoryBackend::fake_oid(n)
}

#[test]
fn log_of_all_refs() {
    let mut repo = sample();
    // In the sample everything is reachable from HEAD: same log.
    assert_eq!(
        log(&repo, LogQuery::all().from_all_refs()),
        log(&repo, LogQuery::all())
    );

    let base = ref_target(&repo, "refs/heads/feature/graph");
    let branch = extra_commit(&mut repo, 90, base, "Only on a branch");
    repo.set_branch("unmerged", branch);
    let remote = extra_commit(&mut repo, 91, base, "Only on the remote");
    repo.set_remote_branch("origin/other", remote);
    let tag = extra_commit(&mut repo, 92, base, "Only on the tag");
    repo.set_tag("only-tag", tag, None);
    let stash = extra_commit(&mut repo, 93, base, "Stash");
    repo.set_stash(stash);

    let all = log(&repo, LogQuery::all().from_all_refs());
    let ids: BTreeSet<Oid> = all.iter().map(|c| c.id).collect();
    assert_eq!(all.len(), 12 + 3, "{:?}", summaries(&all));
    for id in [branch, remote, tag] {
        assert!(ids.contains(&id));
    }
    assert!(!ids.contains(&stash), "the stash is not in the graph");
    // Without `all` nothing changes.
    assert_eq!(log(&repo, LogQuery::all()).len(), 12);
    // Children before parents, the most recent first.
    assert_eq!(all[0].id, tag);
    for (i, c) in all.iter().enumerate() {
        for p in &c.parents {
            assert!(all[i + 1..].iter().any(|later| later.id == *p));
        }
    }
    // Contiguous pages.
    let pages: Vec<Commit> = (0..4)
        .flat_map(|p| log(&repo, LogQuery::page(p * 4, 4).from_all_refs()))
        .collect();
    assert_eq!(pages, all);

    // Detached HEAD on a commit that no ref reaches.
    let detached = extra_commit(&mut repo, 94, base, "detached HEAD");
    repo.detach_head(detached);
    let all = log(&repo, LogQuery::all().from_all_refs());
    assert_eq!(all[0].id, detached);
    assert_eq!(all.len(), 12 + 4);
}

#[test]
fn log_of_all_refs_without_commits() {
    let repo = MemoryBackend::new();
    assert_eq!(log(&repo, LogQuery::all().from_all_refs()), []);
    // HEAD without commits but another branch exists: the log is that of the branch.
    let mut repo = sample();
    repo.set_head_branch("orfano");
    assert_eq!(
        log(&repo, LogQuery::all().from_all_refs()),
        log(&repo, LogQuery::all().from_rev("main"))
    );
    assert_eq!(log(&repo, LogQuery::all()), []);
}

#[test]
fn log_rev_and_all_together_are_an_error() {
    let repo = sample();
    let err = repo
        .log(&LogQuery::all().from_rev("main").from_all_refs())
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::InvalidParams { id: "repo.log", reason } if reason.contains("alternatives")),
        "{err}"
    );
}

fn changed(repo: &MemoryBackend, rev: &str) -> Vec<(Change, String, Option<String>)> {
    repo.show_commit(rev)
        .unwrap()
        .files
        .iter()
        .map(|f| {
            (
                f.change,
                f.path.to_string(),
                f.original.as_ref().map(ToString::to_string),
            )
        })
        .collect()
}

fn commit_by_summary(repo: &MemoryBackend, summary: &str) -> Commit {
    log(repo, LogQuery::all())
        .into_iter()
        .find(|c| c.summary == summary)
        .unwrap_or_else(|| panic!("missing {summary}"))
}

#[test]
fn details_of_the_head_commit() {
    let repo = sample();
    let details = repo.show_commit("HEAD").unwrap();
    let head = &log(&repo, LogQuery::page(0, 1))[0];
    assert_eq!(&details.commit, head);
    assert_eq!(details.compared_to, Some(head.parents[0]));
    assert_eq!(
        changed(&repo, "HEAD"),
        [
            (
                Change::Renamed,
                "src/bin/app.rs".to_owned(),
                Some("src/main.rs".to_owned())
            ),
            (Change::Modified, "src/lib.rs".to_owned(), None),
            (Change::Added, "src/panel.rs".to_owned(), None),
        ]
    );
    let renamed = &details.files[0];
    assert_eq!(renamed.score, Some(92));
    assert_ne!(renamed.old_oid, renamed.new_oid);
    // Same commit by abbreviated id, branch name or tag.
    let short = head.id.to_string()[..6].to_owned();
    assert_eq!(repo.show_commit(&short).unwrap(), details);
    assert_eq!(repo.show_commit("main").unwrap(), details);
}

#[test]
fn details_root_merge_and_deletion() {
    let repo = sample();
    let root = commit_by_summary(&repo, "First commit");
    let details = repo.show_commit(&root.id.to_string()).unwrap();
    assert_eq!(details.compared_to, None);
    for f in &details.files {
        assert_eq!((f.change, f.old_mode), (Change::Added, 0));
        assert_eq!(f.old_oid, Oid::from_bytes([0; Oid::LEN]));
    }

    let structure = commit_by_summary(&repo, "Project structure");
    let deleted: Vec<ChangedFile> = repo
        .show_commit(&structure.id.to_string())
        .unwrap()
        .files
        .into_iter()
        .filter(|f| f.change == Change::Deleted)
        .collect();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].path.to_str(), Some(".gitkeep"));
    assert_eq!(deleted[0].new_mode, 0);
    // The deleted content is the one added by the first commit.
    let added = details
        .files
        .iter()
        .find(|f| f.path.to_str() == Some(".gitkeep"))
        .unwrap();
    assert_eq!(deleted[0].old_oid, added.new_oid);

    // The merge is compared with the first parent: the files brought in by `feature/graph`, with
    // the
    // content they have on the branch.
    let merge = repo.show_commit("v0.1.0").unwrap();
    assert!(merge.commit.is_merge());
    assert_eq!(merge.compared_to, Some(merge.commit.parents[0]));
    let feature = repo.show_commit("feature/graph").unwrap();
    let cargo = |files: &[ChangedFile]| {
        files
            .iter()
            .find(|f| f.path.to_str() == Some("Cargo.toml"))
            .unwrap()
            .new_oid
    };
    assert_eq!(cargo(&merge.files), cargo(&feature.files));
    let paths: Vec<_> = merge.files.iter().map(|f| f.path.to_string()).collect();
    assert_eq!(paths, ["Cargo.toml", "src/graph.rs", "tests/graph.rs"]);
}

#[test]
fn details_errors_and_commit_without_files() {
    let repo = sample();
    for rev in ["missing", "", "abc"] {
        let err = repo.show_commit(rev).unwrap_err();
        assert!(
            matches!(err, EngineError::UnknownRev(ref r) if r == rev),
            "{err}"
        );
    }
    let empty = MemoryBackend::new();
    assert!(matches!(
        empty.show_commit("HEAD"),
        Err(EngineError::UnknownRev(_))
    ));

    // A commit without `set_changes` changes no files; a ref to a missing commit is not
    // shown.
    let mut repo = MemoryBackend::new();
    let mut commit = sample().show_commit("HEAD").unwrap().commit;
    commit.parents.clear();
    repo.insert_commit(commit.clone());
    repo.set_branch("main", commit.id);
    repo.set_branch("fantasma", MemoryBackend::fake_oid(4242));
    let details = repo.show_commit("main").unwrap();
    assert!(details.files.is_empty());
    assert_eq!(details.compared_to, None);
    assert!(matches!(
        repo.show_commit("fantasma"),
        Err(EngineError::UnknownRev(_))
    ));
}
