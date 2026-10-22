use super::{GitBackend, LogQuery, MemoryBackend, MemoryOpener, RepoOpener};
use crate::EngineError;
use project_domain::{Change, Commit, Head, Oid, RefKind, Upstream};
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
        .unwrap_or_else(|| panic!("manca {name}"))
        .target
}

#[test]
fn head_su_main_all_ultimo_commit() {
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
    assert_eq!(first.summary, "Pannello dei commit");
}

#[test]
fn refs_in_ordine_di_nome_con_tipi_e_upstream() {
    let repo = sample();
    let refs = repo.refs().unwrap();
    let names: Vec<_> = refs.iter().map(|r| (r.name.as_str(), r.kind)).collect();
    assert_eq!(
        names,
        [
            ("refs/heads/feature/grafo", RefKind::LocalBranch),
            ("refs/heads/main", RefKind::LocalBranch),
            ("refs/remotes/origin/feature/grafo", RefKind::RemoteBranch),
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
        by_name("refs/heads/feature/grafo").upstream,
        Some(Upstream {
            name: "refs/remotes/origin/feature/grafo".into(),
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
        by_name("refs/heads/feature/grafo").target,
        by_name("refs/remotes/origin/feature/grafo").target
    );
}

#[test]
fn il_tag_annotato_punta_al_merge() {
    let repo = sample();
    let tag = repo
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.kind == RefKind::Tag)
        .unwrap();
    assert_eq!(tag.short_name(), "v0.1.0");
    let tag_object = tag.tag_object.expect("tag annotato");
    assert_ne!(tag_object, tag.target);
    let merge = log(&repo, LogQuery::default().from_rev("v0.1.0"));
    assert_eq!(merge[0].id, tag.target);
    assert!(merge[0].is_merge());
}

#[test]
fn log_completo_figli_prima_dei_genitori() {
    let repo = sample();
    let all = log(&repo, LogQuery::all());
    assert_eq!(all.len(), 12);
    assert_eq!(
        summaries(&all),
        [
            "Pannello dei commit",
            "Parser dei ref",
            "Merge branch 'feature/grafo'",
            "Test di proprietà per le corsie",
            "Parser dello stato",
            "Corsie: riusa le colonne libere",
            "Correggi un refuso nel README",
            "Corsie del grafo: prima versione",
            "Parser del log",
            "Struttura del progetto",
            "Aggiungi il README",
            "Primo commit",
        ]
    );
    let ids: BTreeSet<_> = all.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), all.len(), "nessun commit ripetuto");
    for (i, commit) in all.iter().enumerate() {
        for parent in &commit.parents {
            let pos = all.iter().position(|c| c.id == *parent).unwrap();
            assert!(pos > i, "{} prima del figlio {}", parent, commit.id);
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
fn paginazione_con_skip_e_limit() {
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
    // Le pagine consecutive ricompongono il log.
    let pages: Vec<Commit> = (0..4)
        .flat_map(|p| log(&repo, LogQuery::page(p * 5, 5)))
        .collect();
    assert_eq!(pages, all);
}

#[test]
fn log_da_un_branch_non_include_main() {
    let repo = sample();
    let feature = log(&repo, LogQuery::default().from_rev("feature/grafo"));
    assert_eq!(
        summaries(&feature),
        [
            "Test di proprietà per le corsie",
            "Corsie: riusa le colonne libere",
            "Corsie del grafo: prima versione",
            "Parser del log",
            "Struttura del progetto",
            "Aggiungi il README",
            "Primo commit",
        ]
    );
    let page = log(&repo, LogQuery::page(1, 2).from_rev("feature/grafo"));
    assert_eq!(page, feature[1..3]);
}

#[test]
fn rev_per_nome_completo_breve_head_e_id() {
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
    // Da un commit del lato di main prima del merge: niente commit di feature/grafo.
    assert_eq!(
        summaries(&log(&repo, LogQuery::default().from_rev(target.short()))),
        [
            "Correggi un refuso nel README",
            "Parser del log",
            "Struttura del progetto",
            "Aggiungi il README",
            "Primo commit",
        ]
    );
}

#[test]
fn rev_sconosciuta_e_un_errore() {
    let repo = sample();
    for rev in ["nessuno", "origin/nessuno", "", "abc", "0000000", "HEAD~1"] {
        let err = repo.log(&LogQuery::default().from_rev(rev)).expect_err(rev);
        assert!(
            matches!(&err, EngineError::UnknownRev(r) if r == rev),
            "{err}"
        );
    }
}

#[test]
fn il_merge_ha_due_genitori() {
    let repo = sample();
    let all = log(&repo, LogQuery::all());
    let merges: Vec<_> = all.iter().filter(|c| c.is_merge()).collect();
    assert_eq!(merges.len(), 1);
    let merge = merges[0];
    assert_eq!(merge.parents.len(), 2);
    let summary_of = |id: &Oid| all.iter().find(|c| c.id == *id).unwrap().summary.as_str();
    // Primo genitore: il lato di main; secondo: la punta del branch unito.
    assert_eq!(summary_of(&merge.parents[0]), "Parser dello stato");
    assert_eq!(
        merge.parents[1],
        ref_target(&repo, "refs/heads/feature/grafo")
    );
}

#[test]
fn stato_non_pulito_con_upstream_breve() {
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
            ("docs/guida.md".to_owned(), "A.".to_owned()),
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
fn repo_vuoto() {
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
fn head_staccata_e_upstream_sparito() {
    let mut repo = sample();
    let target = log(&repo, LogQuery::all())[4].id;
    repo.set_branch("vecchio", target);
    repo.set_upstream("vecchio", "origin/vecchio");
    let vecchio = repo
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.short_name() == "vecchio")
        .unwrap();
    assert_eq!(
        vecchio.upstream,
        Some(Upstream {
            name: "refs/remotes/origin/vecchio".into(),
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
fn ahead_e_behind_dal_grafo() {
    let mut repo = sample();
    // origin/main va avanti su un commit che main non ha: 1 avanti, 1 indietro.
    let base = ref_target(&repo, "refs/remotes/origin/main");
    let mut extra = log(&repo, LogQuery::page(0, 1))[0].clone();
    extra.id = MemoryBackend::fake_oid(99);
    extra.parents = vec![base];
    extra.summary = "Commit solo sul remoto".into();
    repo.insert_commit(extra.clone());
    repo.set_remote_branch("origin/main", extra.id);
    let status = repo.status().unwrap();
    let upstream = status.upstream.unwrap();
    assert_eq!((upstream.ahead, upstream.behind), (Some(1), Some(1)));
}

#[test]
fn id_finti_deterministici_e_distinti() {
    assert_eq!(MemoryBackend::fake_oid(7), MemoryBackend::fake_oid(7));
    let ids: BTreeSet<_> = (0..10_000).map(MemoryBackend::fake_oid).collect();
    assert_eq!(ids.len(), 10_000);
    assert!(!ids.contains(&Oid::from_bytes([0; Oid::LEN])));
    // L'esempio è lo stesso a ogni costruzione.
    assert_eq!(MemoryBackend::sample(), MemoryBackend::sample());
}

#[test]
fn opener_in_memoria() {
    let opener = MemoryOpener::sample();
    let repo: Box<dyn GitBackend> = opener.open(MemoryOpener::SAMPLE_PATH).unwrap();
    assert_eq!(repo.log(&LogQuery::all()).unwrap().len(), 12);
    let err = opener.open("/altro").err().unwrap();
    assert!(
        matches!(&err, EngineError::RepoNotFound { path, .. } if path == "/altro"),
        "{err}"
    );
    assert!(err.to_string().contains(MemoryOpener::SAMPLE_PATH), "{err}");

    let mut opener = MemoryOpener::new();
    assert!(opener.open("vuoto").is_err());
    opener.insert("vuoto", MemoryBackend::new());
    assert_eq!(opener.paths().collect::<Vec<_>>(), ["vuoto"]);
    assert!(opener.open("vuoto").unwrap().refs().unwrap().is_empty());
}

#[test]
fn trait_object_send_e_sync() {
    fn send<T: Send + ?Sized>() {}
    fn send_sync<T: Send + Sync + ?Sized>() {}
    send::<dyn GitBackend>();
    send::<Box<dyn GitBackend>>();
    send_sync::<dyn RepoOpener>();
    send_sync::<MemoryOpener>();
}

#[test]
fn log_query_da_json() {
    let q: LogQuery = serde_json::from_str("{}").unwrap();
    assert_eq!(q, LogQuery::all());
    let q: LogQuery = serde_json::from_str(r#"{"skip":2,"limit":3,"rev":"main"}"#).unwrap();
    assert_eq!(q, LogQuery::page(2, 3).from_rev("main"));
    let q: LogQuery = serde_json::from_str(r#"{"limit":null}"#).unwrap();
    assert_eq!(q.limit, None);
    assert!(serde_json::from_str::<LogQuery>(r#"{"count":1}"#).is_err());
    assert!(serde_json::from_str::<LogQuery>(r#"{"skip":-1}"#).is_err());
}
