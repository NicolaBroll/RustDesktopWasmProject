//! Integration tests of `repo.search` on the `CliBackend`: regex characters taken literally,
//! non-UTF-8 message, author by name and by email, short hash, OR of the fields, `path` with a
//! rename (no following), `all` with an unmerged branch, contiguous pages, a text that looks
//! like an option.

mod common;

use common::{Repo, repo_with_history, repo_with_renamed_file};
use project_domain::Oid;
use project_engine::{EngineError, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn session(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

/// `(ids, total, has_more)` of `repo.search` with `params`.
fn search(repo: &Repo, params: Value) -> (Vec<Oid>, u64, bool) {
    let (r, mut s) = session(repo);
    let out = r
        .run("repo.search", &mut s, params.clone())
        .unwrap_or_else(|e| panic!("{params}: {e}"));
    let ids = out["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().parse().unwrap())
        .collect();
    (
        ids,
        out["total"].as_u64().unwrap(),
        out["has_more"].as_bool().unwrap(),
    )
}

fn ids(repo: &Repo, params: Value) -> Vec<Oid> {
    search(repo, params).0
}

/// The order of `git log --date-order` from HEAD (or all refs), filtered to `wanted`.
fn in_log_order(repo: &Repo, all: bool, wanted: &[Oid]) -> Vec<Oid> {
    let mut args = vec!["log", "--format=%H", "--date-order"];
    if all {
        args.extend(["--branches", "--remotes", "--tags"]);
    }
    args.push("HEAD");
    repo.git(&args)
        .lines()
        .map(|l| l.parse().unwrap())
        .filter(|id| wanted.contains(id))
        .collect()
}

#[test]
fn regex_characters_are_literal() {
    let repo = Repo::new();
    let literal = repo.commit_file("a.txt", "1\n", "Fix a.b*[ parser");
    repo.commit_file("a.txt", "2\n", "Fix aXbbb parser");
    let found = ids(&repo, json!({ "query": "A.B*[", "in": ["message"] }));
    assert_eq!(found, [literal]);
    assert!(ids(&repo, json!({ "query": "a.b+", "in": ["message"] })).is_empty());
    // Text that looks like an option: only the value of `--grep=`/`--author=`.
    let dash = repo.commit_file("a.txt", "3\n", "--all is not an option here");
    assert_eq!(ids(&repo, json!({ "query": "--all" })), [dash]);
    assert!(ids(&repo, json!({ "query": "--format=%s" })).is_empty());
}

#[test]
fn non_utf8_message() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "1\n", "plain");
    let file = repo.path().join(".git").join("MSG");
    std::fs::write(&file, b"caf\xe9 needle\n\nbody \xff\xfe more\n").unwrap();
    repo.write("a.txt", "2\n");
    repo.stage(&["a.txt"]);
    repo.git(&["commit", "--quiet", "-F", file.to_str().unwrap()]);
    let latin = repo.rev("HEAD");
    assert_eq!(ids(&repo, json!({ "query": "NEEDLE" })), [latin]);
    assert_eq!(
        ids(&repo, json!({ "query": "more", "in": ["message"] })),
        [latin]
    );
}

#[test]
fn author_by_name_and_by_email() {
    let h = repo_with_history();
    let repo = &h.repo;
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    repo.git(&[
        "commit",
        "--quiet",
        "-m",
        "by carla",
        "--author=Carla Neri <carla@example.org>",
    ]);
    let carla = repo.rev("HEAD");
    let by_name = ids(repo, json!({ "query": "carla neri", "in": ["author"] }));
    assert_eq!(by_name, [carla]);
    let by_email = ids(repo, json!({ "query": "@EXAMPLE.ORG", "in": ["author"] }));
    assert_eq!(by_email, [carla]);
    // The committer (Bob) is not the author.
    assert!(ids(repo, json!({ "query": "bob", "in": ["author"] })).is_empty());
    // Ada: every other commit, in log order.
    let ada = ids(repo, json!({ "query": "ada@", "in": ["author"] }));
    assert_eq!(
        ada,
        in_log_order(repo, false, &[h.m, h.c2, h.f1, h.f2, h.c1])
    );
    // OR of the fields (git alone would AND `--grep` and `--author`): "e" is in the message of
    // every commit but "by carla", whose author "Carla Neri" has it.
    assert!(!ids(repo, json!({ "query": "e", "in": ["message"] })).contains(&carla));
    let either = ids(repo, json!({ "query": "e", "in": ["message", "author"] }));
    assert_eq!(
        either,
        in_log_order(repo, false, &[carla, h.m, h.c2, h.f1, h.f2, h.c1])
    );
}

#[test]
fn short_hash() {
    let h = repo_with_history();
    let hex = h.f1.to_string();
    let short = hex[..7].to_ascii_uppercase();
    assert_eq!(
        ids(&h.repo, json!({ "query": short, "in": ["id"] })),
        [h.f1]
    );
    assert_eq!(ids(&h.repo, json!({ "query": hex, "in": ["id"] })), [h.f1]);
    // Not a prefix: no match.
    assert!(ids(&h.repo, json!({ "query": &hex[1..9], "in": ["id"] })).is_empty());
    // Without `id` among the fields the hash does not match.
    assert!(ids(&h.repo, json!({ "query": &hex[..7], "in": ["message"] })).is_empty());
}

#[test]
fn path_with_a_rename_is_not_followed() {
    let f = repo_with_renamed_file();
    let repo = &f.repo;
    // Ada is the author of every commit: only `path` filters.
    let all = json!({ "query": "ada", "in": ["author"] });
    let mut new = all.clone();
    new["path"] = json!("b.txt");
    let mut old = all.clone();
    old["path"] = json!("a.txt");
    // b.txt: the rename creates it; the merge brings `five` into main.
    assert_eq!(
        ids(repo, new),
        in_log_order(repo, false, &[f.merge, f.one, f.five, f.four, f.rename])
    );
    // a.txt: up to the rename, which deletes it.
    assert_eq!(
        ids(repo, old),
        in_log_order(repo, false, &[f.rename, f.two, f.add])
    );
    // `log.follow` does not change it.
    repo.git(&["config", "log.follow", "true"]);
    let mut new = all;
    new["path"] = json!("b.txt");
    assert!(!ids(repo, new).contains(&f.two));
}

#[test]
fn path_is_literal() {
    let repo = Repo::new();
    let star = repo.commit_file("*.txt", "1\n", "star");
    repo.commit_file("a.txt", "1\n", "plain");
    let found = ids(
        &repo,
        json!({ "query": "ada", "in": ["author"], "path": "*.txt" }),
    );
    assert_eq!(found, [star]);
}

#[test]
fn all_with_an_unmerged_branch() {
    let h = repo_with_history();
    let repo = &h.repo;
    repo.checkout("feature");
    let only = repo.commit_file("c.txt", "c\n", "only on feature");
    repo.checkout("main");
    assert!(ids(repo, json!({ "query": "only on" })).is_empty());
    assert_eq!(
        ids(repo, json!({ "query": "only on", "all": true })),
        [only]
    );
    let feature = ids(
        repo,
        json!({ "query": "feature", "all": true, "in": ["message"] }),
    );
    assert_eq!(feature, in_log_order(repo, true, &[only, h.m, h.f1, h.f2]));
}

#[test]
fn contiguous_pages() {
    let repo = Repo::new();
    for i in 0..9 {
        repo.commit_file("a.txt", &format!("{i}\n"), &format!("step {i}"));
    }
    repo.commit_file("a.txt", "x\n", "unrelated");
    let (all, total, more) = search(&repo, json!({ "query": "STEP" }));
    assert_eq!((all.len(), total, more), (9, 9, false));
    let mut joined = Vec::new();
    for skip in (0..9).step_by(4) {
        let (page, t, more) = search(&repo, json!({ "query": "step", "skip": skip, "limit": 4 }));
        assert_eq!(t, 9);
        assert_eq!(more, skip + 4 < 9);
        joined.extend(page);
    }
    assert_eq!(joined, all);
    assert_eq!(all, in_log_order(&repo, false, &all));
}

#[test]
fn empty_repo_and_errors() {
    let repo = Repo::new();
    assert_eq!(search(&repo, json!({ "query": "x" })), (vec![], 0, false));
    let (r, mut s) = session(&repo);
    for params in [json!({ "query": "" }), json!({ "query": "a\nb" })] {
        let e = r.run("repo.search", &mut s, params).unwrap_err();
        assert!(matches!(e, EngineError::InvalidParams { .. }), "{e}");
    }
}
