//! Integration tests of `repo.log` with `"all": true` on the `CliBackend`: the log starts from
//! all local and remote branches, the tags and HEAD. Commit only on an unmerged branch, remote
//! branches just fetched (even with an unrelated history), stash and backup refs excluded,
//! detached HEAD, HEAD without commits, contiguous pages, `rev` and `all` together.

mod common;

use common::{Repo, repo_with_all_refs, repo_with_history};
use project_domain::{Commit, Oid, layout_graph};
use project_engine::{EngineError, GitBackend, LogQuery, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn ids(commits: &[Commit]) -> Vec<Oid> {
    commits.iter().map(|c| c.id).collect()
}

fn log_all(repo: &Repo) -> Vec<Commit> {
    repo.open().log(&LogQuery::all().from_all_refs()).unwrap()
}

/// The order that git itself gives with the same starting points.
fn git_order(repo: &Repo) -> Vec<Oid> {
    repo.git(&[
        "log",
        "--format=%H",
        "--date-order",
        "--branches",
        "--remotes",
        "--tags",
        "HEAD",
    ])
    .lines()
    .map(|l| l.parse().unwrap())
    .collect()
}

/// Children before parents, and the graph lays out without errors.
fn check_graph(commits: &[Commit]) {
    for (i, c) in commits.iter().enumerate() {
        for p in &c.parents {
            if let Some(j) = commits.iter().position(|x| x.id == *p) {
                assert!(j > i, "{} before its child {}", p, c.id);
            }
        }
    }
    let layout = layout_graph(commits);
    assert_eq!(layout.rows.len(), commits.len());
    for row in &layout.rows {
        assert!(row.column < layout.width);
    }
}

#[test]
fn commit_on_an_unmerged_branch() {
    let h = repo_with_history();
    h.repo.checkout("feature");
    let only = h.repo.commit_file("c.txt", "c\n", "only on feature");
    h.repo.checkout("main");

    let head = h.repo.open().log(&LogQuery::all()).unwrap();
    assert!(!ids(&head).contains(&only));
    let all = log_all(&h.repo);
    assert_eq!(ids(&all), git_order(&h.repo));
    assert_eq!(all.len(), head.len() + 1);
    assert_eq!(all[0].id, only, "the most recent first");
    assert_eq!(all[0].summary, "only on feature");
    check_graph(&all);
}

#[test]
fn fetched_remote_branches_even_unrelated() {
    let h = repo_with_history();
    let remote = Repo::bare();
    // Another history, without common commits: a second root in the graph.
    let other = Repo::new();
    other.add_remote("origin", &remote);
    let r1 = other.commit_file("x.txt", "x\n", "other root");
    let r2 = other.commit_file("x.txt", "xx\n", "other history");
    other.push("origin", "HEAD", "extra");
    h.repo.add_remote("origin", &remote);

    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": h.repo.path_str() }))
        .unwrap();
    r.run("remote.fetch", &mut s, json!({ "remote": "origin" }))
        .unwrap();
    let page = |s: &mut Session, params: Value| -> Vec<Oid> {
        let out = r.run("repo.log", s, params).unwrap();
        serde_json::from_value::<Vec<Commit>>(out["commits"].clone())
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect()
    };
    let head = page(&mut s, json!({}));
    assert!(!head.contains(&r2));
    let all = page(&mut s, json!({ "all": true }));
    assert_eq!(all, git_order(&h.repo));
    assert!(all.contains(&r1) && all.contains(&r2));
    assert_eq!(all.len(), head.len() + 2);
    // `origin/extra` is on the commit just fetched: it opens without "commit not loaded".
    let show = r
        .run("commit.show", &mut s, json!({ "id": "origin/extra" }))
        .unwrap();
    assert_eq!(show["commit"]["id"], r2.to_string());
    check_graph(&log_all(&h.repo));
}

#[test]
fn stash_and_backup_excluded() {
    let (h, _origin) = repo_with_all_refs();
    let stash = h.repo.rev("refs/stash");
    // A commit reachable only from a backup ref.
    h.repo.branch("tmp");
    h.repo.checkout("tmp");
    let backup = h.repo.commit_file("z.txt", "z\n", "only in the backup");
    h.repo.checkout("main");
    h.repo.git(&[
        "update-ref",
        "refs/project/backup/delete-1",
        &backup.to_string(),
    ]);
    h.repo.git(&["branch", "-D", "tmp"]);
    // And one from a note (`refs/notes/commits`), also excluded.
    h.repo.git(&["notes", "add", "-m", "note", "HEAD"]);
    let note = h.repo.rev("refs/notes/commits");

    let all = ids(&log_all(&h.repo));
    for (id, what) in [(stash, "stash"), (backup, "backup"), (note, "note")] {
        assert!(!all.contains(&id), "{what} in the graph");
    }
    // The stash is a child of HEAD: its internal parents (the index) are not there either.
    let stash_index = h.repo.rev("refs/stash^2");
    assert!(!all.contains(&stash_index));
    assert_eq!(all, ids(&h.repo.open().log(&LogQuery::all()).unwrap()));
}

#[test]
fn detached_head_appears() {
    let h = repo_with_history();
    h.repo.detach(h.c1);
    let detached = h.repo.commit_file("d.txt", "d\n", "on detached HEAD");

    let all = log_all(&h.repo);
    assert_eq!(all[0].id, detached);
    assert_eq!(ids(&all), git_order(&h.repo));
    assert!(ids(&all).contains(&h.m));
    check_graph(&all);
}

#[test]
fn head_without_commits_and_empty_repo() {
    let repo = Repo::new();
    assert!(log_all(&repo).is_empty());

    let h = repo_with_history();
    h.repo.git(&["checkout", "--quiet", "--orphan", "fresh"]);
    assert!(h.repo.open().log(&LogQuery::all()).unwrap().is_empty());
    let all = log_all(&h.repo);
    assert_eq!(all[0].id, h.m);
    assert_eq!(all.len(), 5);
}

#[test]
fn tags_on_objects_that_are_not_commits() {
    let h = repo_with_history();
    let blob = h.repo.git(&["hash-object", "-w", "a.txt"]);
    h.repo.tag("blob", &blob);
    h.repo.annotated_tag("tree", "HEAD^{tree}", "a tree");
    assert_eq!(
        ids(&log_all(&h.repo)),
        ids(&h.repo.open().log(&LogQuery::all()).unwrap())
    );
}

#[test]
fn contiguous_pages() {
    let h = repo_with_history();
    for (branch, file) in [("a", "a1.txt"), ("b", "b1.txt"), ("c", "c1.txt")] {
        h.repo
            .git(&["checkout", "--quiet", "-b", branch, &h.c1.to_string()]);
        h.repo.commit_file(file, "1\n", &format!("{branch} 1"));
        h.repo.commit_file(file, "2\n", &format!("{branch} 2"));
    }
    h.repo.checkout("main");
    let backend = h.repo.open();
    let all = log_all(&h.repo);
    assert_eq!(all.len(), 5 + 6);
    assert_eq!(ids(&all), git_order(&h.repo));

    for size in [1, 2, 3, 4] {
        let mut pages = Vec::new();
        let mut skip = 0;
        loop {
            let page = backend
                .log(&LogQuery::page(skip, size).from_all_refs())
                .unwrap();
            if page.is_empty() {
                break;
            }
            skip += page.len();
            pages.extend(page);
        }
        assert_eq!(pages, all, "pages of {size}");
    }

    // With the `Registry`: `has_more` false only on the last page.
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": h.repo.path_str() }))
        .unwrap();
    let out = r
        .run(
            "repo.log",
            &mut s,
            json!({ "all": true, "skip": 8, "limit": 3 }),
        )
        .unwrap();
    assert_eq!(out["has_more"], false);
    assert_eq!(out["commits"].as_array().unwrap().len(), 3);
    let out = r
        .run(
            "repo.log",
            &mut s,
            json!({ "all": true, "skip": 7, "limit": 3 }),
        )
        .unwrap();
    assert_eq!(out["has_more"], true);
}

#[test]
fn rev_and_all_together_are_an_error() {
    let h = repo_with_history();
    let err = h
        .repo
        .open()
        .log(&LogQuery::all().from_rev("main").from_all_refs())
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::InvalidParams { id: "repo.log", .. }),
        "{err}"
    );
}
