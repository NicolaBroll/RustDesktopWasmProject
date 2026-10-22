//! Test di integrazione di `repo.log` con `"all": true` sul `CliBackend`: il log parte da tutti i
//! branch locali e remoti, i tag e HEAD. Commit solo su un branch non unito, branch remoti
//! appena scaricati (anche con una storia scollegata), stash e ref di backup esclusi, HEAD
//! staccata, HEAD senza commit, pagine contigue, `rev` e `all` insieme.

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

/// L'ordine che dà git stesso con le stesse partenze.
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

/// Figli prima dei genitori, e il grafo si dispone senza errori.
fn check_graph(commits: &[Commit]) {
    for (i, c) in commits.iter().enumerate() {
        for p in &c.parents {
            if let Some(j) = commits.iter().position(|x| x.id == *p) {
                assert!(j > i, "{} prima del figlio {}", p, c.id);
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
fn commit_di_un_branch_non_unito() {
    let h = repo_with_history();
    h.repo.checkout("feature");
    let solo = h.repo.commit_file("c.txt", "c\n", "solo su feature");
    h.repo.checkout("main");

    let head = h.repo.open().log(&LogQuery::all()).unwrap();
    assert!(!ids(&head).contains(&solo));
    let all = log_all(&h.repo);
    assert_eq!(ids(&all), git_order(&h.repo));
    assert_eq!(all.len(), head.len() + 1);
    assert_eq!(all[0].id, solo, "il più recente prima");
    assert_eq!(all[0].summary, "solo su feature");
    check_graph(&all);
}

#[test]
fn branch_remoti_scaricati_anche_scollegati() {
    let h = repo_with_history();
    let remote = Repo::bare();
    // Un'altra storia, senza commit in comune: una seconda radice nel grafo.
    let other = Repo::new();
    other.add_remote("origin", &remote);
    let r1 = other.commit_file("x.txt", "x\n", "altra radice");
    let r2 = other.commit_file("x.txt", "xx\n", "altra storia");
    other.push("origin", "HEAD", "altro");
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
    // `origin/altro` è sul commit appena scaricato: si apre senza "commit non caricato".
    let show = r
        .run("commit.show", &mut s, json!({ "id": "origin/altro" }))
        .unwrap();
    assert_eq!(show["commit"]["id"], r2.to_string());
    check_graph(&log_all(&h.repo));
}

#[test]
fn stash_e_backup_esclusi() {
    let (h, _origin) = repo_with_all_refs();
    let stash = h.repo.rev("refs/stash");
    // Un commit raggiungibile solo da un ref di backup.
    h.repo.branch("tmp");
    h.repo.checkout("tmp");
    let backup = h.repo.commit_file("z.txt", "z\n", "solo nel backup");
    h.repo.checkout("main");
    h.repo.git(&[
        "update-ref",
        "refs/project/backup/delete-1",
        &backup.to_string(),
    ]);
    h.repo.git(&["branch", "-D", "tmp"]);
    // E uno da una nota (`refs/notes/commits`), anche lei fuori.
    h.repo.git(&["notes", "add", "-m", "nota", "HEAD"]);
    let note = h.repo.rev("refs/notes/commits");

    let all = ids(&log_all(&h.repo));
    for (id, what) in [(stash, "stash"), (backup, "backup"), (note, "nota")] {
        assert!(!all.contains(&id), "{what} nel grafo");
    }
    // Lo stash è figlio di HEAD: anche i suoi genitori interni (l'indice) non ci sono.
    let stash_index = h.repo.rev("refs/stash^2");
    assert!(!all.contains(&stash_index));
    assert_eq!(all, ids(&h.repo.open().log(&LogQuery::all()).unwrap()));
}

#[test]
fn head_staccata_compare() {
    let h = repo_with_history();
    h.repo.detach(h.c1);
    let detached = h.repo.commit_file("d.txt", "d\n", "su HEAD staccata");

    let all = log_all(&h.repo);
    assert_eq!(all[0].id, detached);
    assert_eq!(ids(&all), git_order(&h.repo));
    assert!(ids(&all).contains(&h.m));
    check_graph(&all);
}

#[test]
fn head_senza_commit_e_repo_vuoto() {
    let repo = Repo::new();
    assert!(log_all(&repo).is_empty());

    let h = repo_with_history();
    h.repo.git(&["checkout", "--quiet", "--orphan", "nuovo"]);
    assert!(h.repo.open().log(&LogQuery::all()).unwrap().is_empty());
    let all = log_all(&h.repo);
    assert_eq!(all[0].id, h.m);
    assert_eq!(all.len(), 5);
}

#[test]
fn tag_su_oggetti_che_non_sono_commit() {
    let h = repo_with_history();
    let blob = h.repo.git(&["hash-object", "-w", "a.txt"]);
    h.repo.tag("blob", &blob);
    h.repo.annotated_tag("albero", "HEAD^{tree}", "un albero");
    assert_eq!(
        ids(&log_all(&h.repo)),
        ids(&h.repo.open().log(&LogQuery::all()).unwrap())
    );
}

#[test]
fn pagine_contigue() {
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
        assert_eq!(pages, all, "pagine da {size}");
    }

    // Con il `Registry`: `has_more` falso solo sull'ultima pagina.
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
fn rev_e_all_insieme_sono_un_errore() {
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
