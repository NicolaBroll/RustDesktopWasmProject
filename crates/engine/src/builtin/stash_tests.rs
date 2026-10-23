//! Test di `stash.list`, `stash.push`, `stash.apply`, `stash.pop`, `stash.drop` e del loro
//! undo/redo sul `MemoryBackend` (repo di esempio: `README.md` `MM`, `docs/guida.md` `A.`,
//! `src/lib.rs` `.M`, `note.txt` non tracciato).

use crate::{EngineError, GitBackend, MemoryBackend, MemoryOpener, Registry, Session, StashPush};
use project_domain::{
    Change, ChangedFile, Commit, EntryKind, Operation, RepoPath, Signature, StatusEntry,
};
use serde_json::{Value, json};
use std::sync::Arc;

fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::default();
    opener.insert("r", repo);
    let mut s = Session::new(Arc::new(opener));
    let r = Registry::builtin();
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
    (r, s)
}

fn opened() -> (Registry, Session) {
    opened_with(MemoryBackend::sample())
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// `(percorso, X, Y)` di ogni voce dello stato.
fn codes(status: &Value) -> Vec<(String, String, String)> {
    status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["path"].as_str().unwrap().to_owned(),
                e["index"].as_str().unwrap().to_owned(),
                e["worktree"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn code(path: &str, x: &str, y: &str) -> (String, String, String) {
    (path.to_owned(), x.to_owned(), y.to_owned())
}

fn stash_count(r: &Registry, s: &mut Session) -> usize {
    run(r, s, "stash.list", json!({}))["stashes"]
        .as_array()
        .unwrap()
        .len()
}

#[test]
fn push_list_undo_and_redo() {
    let (r, mut s) = opened();
    let before = run(&r, &mut s, "repo.status", json!({}));
    let diff_before = run(&r, &mut s, "diff.worktree", json!({}));
    assert_eq!(
        run(&r, &mut s, "stash.list", json!({})),
        json!({ "stashes": [] })
    );

    let out = run(&r, &mut s, "stash.push", json!({}));
    let stash = &out["stash"];
    let head = before["head"]["Branch"]["target"].clone();
    assert_eq!(stash["index"], 0);
    assert_eq!(stash["name"], "stash@{0}");
    assert_eq!(stash["base"], head);
    assert_eq!(stash["branch"], "main");
    assert_eq!(stash["untracked"], false);
    assert!(
        stash["message"]
            .as_str()
            .unwrap()
            .starts_with("WIP on main: "),
        "{stash}"
    );
    // Restano solo i non tracciati (senza `include_untracked`).
    assert_eq!(
        codes(&out["status"]),
        [code("note.txt", "Untracked", "Untracked")]
    );
    assert_eq!(out["stashes"], json!([stash]));
    assert_eq!(out["can_undo"], true);
    assert!(
        out["next_undo"]
            .as_str()
            .unwrap()
            .starts_with("Metti da parte le modifiche (stash ")
    );
    // `refs/stash` è fra i ref, e il commit dello stash si legge come un commit.
    let refs = run(&r, &mut s, "repo.refs", json!({}));
    let stash_ref = refs
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "refs/stash")
        .unwrap();
    assert_eq!(stash_ref["kind"], "Stash");
    assert_eq!(stash_ref["target"], stash["id"]);
    let show = run(&r, &mut s, "commit.show", json!({ "id": stash["id"] }));
    let files: Vec<&str> = show["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["README.md", "docs/guida.md", "src/lib.rs"]);
    assert_eq!(show["compared_to"], head);

    // Un secondo push non ha niente da mettere da parte.
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("niente da mettere da parte")),
        "{err}"
    );

    // Undo: lo stash torna nei file (anche la divisione fra in stage e no) e sparisce
    // dall'elenco, con il commit in un backup.
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["undone"]["command"], "stash.push");
    assert_eq!(undo["status"], before);
    assert_eq!(undo["backups"], json!(["refs/project/backup/undo-1"]));
    assert_eq!(run(&r, &mut s, "diff.worktree", json!({})), diff_before);
    assert_eq!(stash_count(&r, &mut s), 0);
    assert_eq!(
        undo["undone"]["stash"]["push"],
        json!({ "message": null, "include_untracked": false, "keep_index": false, "paths": null })
    );

    // Redo: di nuovo da parte.
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(codes(&redo["status"]), codes(&out["status"]));
    let list = run(&r, &mut s, "stash.list", json!({}));
    assert_eq!(list["stashes"].as_array().unwrap().len(), 1);
    assert_eq!(redo["redone"]["stash"]["id"], list["stashes"][0]["id"]);
    // E l'undo di nuovo.
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["status"], before);
}

#[test]
fn push_options() {
    // Con i non tracciati e un messaggio.
    let (r, mut s) = opened();
    let out = run(
        &r,
        &mut s,
        "stash.push",
        json!({ "message": "-x a metà", "include_untracked": true }),
    );
    assert_eq!(out["status"]["entries"], json!([]));
    assert_eq!(out["stash"]["message"], "On main: -x a metà");
    assert_eq!(out["stash"]["untracked"], true);
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(
        codes(&undo["status"]),
        [
            code("README.md", "Modified", "Modified"),
            code("docs/guida.md", "Added", "Unmodified"),
            code("src/lib.rs", "Unmodified", "Modified"),
            code("note.txt", "Untracked", "Untracked"),
        ]
    );

    // `keep_index`: quello in stage resta, e il worktree è come l'indice.
    let (r, mut s) = opened();
    let out = run(&r, &mut s, "stash.push", json!({ "keep_index": true }));
    assert_eq!(
        codes(&out["status"]),
        [
            code("README.md", "Modified", "Unmodified"),
            code("docs/guida.md", "Added", "Unmodified"),
            code("note.txt", "Untracked", "Untracked"),
        ]
    );

    // Solo alcuni percorsi.
    let (r, mut s) = opened();
    let out = run(&r, &mut s, "stash.push", json!({ "paths": ["src"] }));
    assert_eq!(
        codes(&out["status"]),
        [
            code("README.md", "Modified", "Modified"),
            code("docs/guida.md", "Added", "Unmodified"),
            code("note.txt", "Untracked", "Untracked"),
        ]
    );
    let show = run(
        &r,
        &mut s,
        "commit.show",
        json!({ "id": out["stash"]["id"] }),
    );
    assert_eq!(show["files"].as_array().unwrap().len(), 1);
    // Un non tracciato chiesto senza `include_untracked`.
    let err = r
        .run("stash.push", &mut s, json!({ "paths": ["note.txt"] }))
        .unwrap_err();
    assert!(err.to_string().contains("include_untracked"), "{err}");
}

#[test]
fn push_refusals() {
    let (r, mut s) = opened();
    for params in [
        json!({ "message": "due\nrighe" }),
        json!({ "paths": [] }),
        json!({ "paths": ["../fuori"] }),
        json!({ "all": true }),
        json!({ "message": 3 }),
        json!({ "keep_index": "sì" }),
    ] {
        let err = r.run("stash.push", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::InvalidParams {
                    id: "stash.push",
                    ..
                }
            ),
            "{params}: {err}"
        );
    }
    let mut repo = MemoryBackend::sample();
    repo.set_operation(Some(Operation::Merge));
    let (r, mut s) = opened_with(repo);
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(stash_count(&r, &mut s), 0);

    let (r, mut s) = opened_with(MemoryBackend::new());
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("non ha ancora commit"), "{err}");
}

#[test]
fn apply_keeps_the_stash_and_pop_drops_it() {
    let (r, mut s) = opened();
    let before = run(&r, &mut s, "repo.status", json!({}));
    let pushed = run(
        &r,
        &mut s,
        "stash.push",
        json!({ "include_untracked": true }),
    );
    let id = pushed["stash"]["id"].clone();

    // Apply senza indice: tutto fuori stage, tranne il file nuovo (come git).
    let out = run(&r, &mut s, "stash.apply", json!({ "expected": id }));
    assert_eq!(out["stash"]["id"], id);
    assert_eq!(out["conflicts"], json!([]));
    assert_eq!(out["dropped"], false);
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["stashes"].as_array().unwrap().len(), 1);
    assert_eq!(
        codes(&out["status"]),
        [
            code("README.md", "Unmodified", "Modified"),
            code("docs/guida.md", "Added", "Unmodified"),
            code("src/lib.rs", "Unmodified", "Modified"),
            code("note.txt", "Untracked", "Untracked"),
        ]
    );
    // Apply e pop non entrano nel diario: il prossimo undo è ancora il push.
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"].as_array().unwrap().len(), 1);
    assert_eq!(history["done"][0]["command"], "stash.push");
    // Di nuovo sopra le stesse modifiche: rifiutato, niente cambia.
    let status = run(&r, &mut s, "repo.status", json!({}));
    let err = r.run("stash.pop", &mut s, json!({})).unwrap_err();
    let EngineError::NotAllowed(message) = &err else {
        panic!("{err}");
    };
    assert!(message.contains("`README.md`"), "{message}");
    assert!(message.contains("`note.txt`"), "{message}");
    assert_eq!(run(&r, &mut s, "repo.status", json!({})), status);
    assert_eq!(stash_count(&r, &mut s), 1);

    // Pop con l'indice da un repo pulito: tutto com'era, lo stash esce con un backup.
    let (r, mut s) = opened();
    run(
        &r,
        &mut s,
        "stash.push",
        json!({ "include_untracked": true }),
    );
    let out = run(&r, &mut s, "stash.pop", json!({ "restore_index": true }));
    assert_eq!(out["status"], before);
    assert_eq!(out["dropped"], true);
    assert_eq!(out["backup"], "refs/project/backup/stash-1");
    assert_eq!(out["stashes"], json!([]));
    assert_eq!(out["warnings"], json!([]));
}

#[test]
fn apply_of_a_moved_stash_or_a_missing_one() {
    let (r, mut s) = opened();
    let err = r.run("stash.apply", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("non ci sono stash"), "{err}");
    let first = run(&r, &mut s, "stash.push", json!({ "paths": ["src"] }));
    let second = run(&r, &mut s, "stash.push", json!({ "paths": ["README.md"] }));
    let list = run(&r, &mut s, "stash.list", json!({}));
    assert_eq!(list["stashes"][0]["id"], second["stash"]["id"]);
    assert_eq!(list["stashes"][1]["id"], first["stash"]["id"]);
    assert_eq!(list["stashes"][1]["name"], "stash@{1}");
    let err = r
        .run("stash.apply", &mut s, json!({ "index": 2 }))
        .unwrap_err();
    assert!(err.to_string().contains("gli stash sono 2"), "{err}");
    // Lo stash visto in posizione 0 ora è in posizione 1.
    let err = r
        .run(
            "stash.pop",
            &mut s,
            json!({ "index": 0, "expected": first["stash"]["id"] }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("ora è `stash@{1}`"), "{err}");
    for params in [
        json!({ "expected": "abc" }),
        json!({ "index": -1 }),
        json!({ "id": "x" }),
        json!({ "confirm": true }),
    ] {
        let err = r.run("stash.apply", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
    // Il pop del più vecchio lascia il più recente in cima.
    let out = run(
        &r,
        &mut s,
        "stash.pop",
        json!({ "index": 1, "expected": first["stash"]["id"] }),
    );
    assert_eq!(out["dropped"], true);
    assert_eq!(out["stashes"][0]["id"], second["stash"]["id"]);
}

/// Il repo di esempio con le modifiche messe da parte, poi un commit che cambia `README.md`
/// su `main`: lo stash va in conflitto su quel file.
fn moved_head() -> (MemoryBackend, String) {
    let mut repo = MemoryBackend::sample();
    let old = match &repo.status().unwrap().entries[0].kind {
        EntryKind::Ordinary(info) => info.oid_head,
        other => panic!("{other:?}"),
    };
    let stash = repo.stash_push(&StashPush::default()).unwrap();
    let head = repo.head().unwrap().target().unwrap();
    let new = MemoryBackend::fake_oid(4242);
    repo.set_blob(new, "README cambiato su main\n");
    let id = MemoryBackend::fake_oid(4243);
    let who = Signature {
        name: "Ada".to_owned(),
        email: "ada@example.com".to_owned(),
        time: 1_800_000_000,
        offset_min: 0,
    };
    repo.insert_commit(Commit {
        id,
        parents: vec![head],
        author: who.clone(),
        committer: who,
        summary: "README".to_owned(),
        body: String::new(),
    });
    repo.set_changes(
        id,
        vec![ChangedFile {
            path: RepoPath::from("README.md"),
            change: Change::Modified,
            original: None,
            score: None,
            old_mode: 0o100644,
            new_mode: 0o100644,
            old_oid: old,
            new_oid: new,
        }],
    );
    repo.set_branch("main", id);
    (repo, stash.id.to_string())
}

#[test]
fn conflicts_are_left_and_pop_keeps_the_stash() {
    let (repo, id) = moved_head();
    let (r, mut s) = opened_with(repo);
    let out = run(&r, &mut s, "stash.pop", json!({ "expected": id }));
    assert_eq!(out["conflicts"], json!(["README.md"]));
    assert_eq!(out["dropped"], false);
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["stashes"][0]["id"], id);
    assert!(
        out["warnings"][0]
            .as_str()
            .unwrap()
            .contains("resta nell'elenco"),
        "{out}"
    );
    let readme = &out["status"]["entries"][0];
    assert_eq!(readme["path"], "README.md");
    assert_eq!(readme["index"], "Unmerged");
    // Gli altri file si applicano.
    assert!(
        codes(&out["status"]).contains(&code("src/lib.rs", "Unmodified", "Modified")),
        "{}",
        out["status"]
    );
    // Con un conflitto aperto niente stash: né apply né push.
    for id in ["stash.apply", "stash.push"] {
        let err = r.run(id, &mut s, json!({})).unwrap_err();
        assert!(err.to_string().contains("in conflitto"), "{id}: {err}");
    }
    // Risolto con lo stage del file, lo stash si elimina a mano.
    run(&r, &mut s, "index.stage", json!({ "paths": ["README.md"] }));
    let out = run(
        &r,
        &mut s,
        "stash.drop",
        json!({ "confirm": true, "expected": id }),
    );
    assert_eq!(out["stashes"], json!([]));
}

#[test]
fn untracked_in_the_way_blocks_the_apply() {
    let mut repo = MemoryBackend::sample();
    repo.stash_push(&StashPush {
        include_untracked: true,
        ..StashPush::default()
    })
    .unwrap();
    // `note.txt` è di nuovo nel worktree.
    repo.set_worktree_file("note.txt", "altro\n");
    repo.set_entries(vec![StatusEntry {
        path: RepoPath::from("note.txt"),
        index: Change::Untracked,
        worktree: Change::Untracked,
        kind: EntryKind::Untracked,
    }]);
    let (r, mut s) = opened_with(repo);
    let before = run(&r, &mut s, "repo.status", json!({}));
    let err = r.run("stash.apply", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`note.txt`")),
        "{err}"
    );
    assert_eq!(run(&r, &mut s, "repo.status", json!({})), before);
}

#[test]
fn drop_needs_confirm_and_undo_puts_it_back_on_top() {
    let (r, mut s) = opened();
    let first = run(&r, &mut s, "stash.push", json!({ "paths": ["src"] }))["stash"].clone();
    let second = run(&r, &mut s, "stash.push", json!({ "message": "readme" }))["stash"].clone();

    let err = r
        .run("stash.drop", &mut s, json!({ "index": 1 }))
        .unwrap_err();
    let EngineError::ConfirmRequired {
        command,
        reason,
        expected,
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(*command, "stash.drop");
    // L'id da ripetere in `expected`, anche nel testo.
    assert_eq!(expected.as_deref(), first["id"].as_str());
    assert!(reason.contains(first["id"].as_str().unwrap()), "{reason}");
    // `confirm` senza `expected` non basta.
    let err = r
        .run("stash.drop", &mut s, json!({ "index": 1, "confirm": true }))
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::InvalidParams {
                id: "stash.drop",
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(stash_count(&r, &mut s), 2);
    assert!(reason.contains("`stash@{1}`"), "{reason}");
    assert!(reason.contains("1 file tracciati"), "{reason}");
    assert_eq!(stash_count(&r, &mut s), 2);
    let err = r
        .run(
            "stash.drop",
            &mut s,
            json!({ "index": 1, "expected": second["id"], "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    let out = run(
        &r,
        &mut s,
        "stash.drop",
        json!({ "index": 1, "expected": first["id"], "confirm": true }),
    );
    assert_eq!(out["stash"]["id"], first["id"]);
    assert_eq!(out["backup"], "refs/project/backup/stash-1");
    assert_eq!(out["stashes"], json!([second]));
    assert_eq!(
        out["next_undo"],
        format!(
            "Elimina stash@{{1}} ({} «{}»)",
            &first["id"].as_str().unwrap()[..7],
            first["message"].as_str().unwrap()
        )
    );
    let entry = &run(&r, &mut s, "edit.history", json!({}))["done"][2];
    assert_eq!(entry["command"], "stash.drop");
    assert_eq!(entry["backup"], "refs/project/backup/stash-1");
    assert_eq!(entry["stash"]["id"], first["id"]);

    // Undo: torna, ma in cima (`git stash store`).
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["undone"]["command"], "stash.drop");
    let list = run(&r, &mut s, "stash.list", json!({}));
    assert_eq!(list["stashes"][0]["id"], first["id"]);
    assert_eq!(list["stashes"][0]["message"], first["message"]);
    assert_eq!(list["stashes"][1]["id"], second["id"]);
    // E si applica ancora con i suoi file.
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(redo["redone"]["command"], "stash.drop");
    assert_eq!(stash_count(&r, &mut s), 1);
    run(&r, &mut s, "edit.undo", json!({}));
    let out = run(&r, &mut s, "stash.pop", json!({ "expected": first["id"] }));
    assert!(
        codes(&out["status"]).contains(&code("src/lib.rs", "Unmodified", "Modified")),
        "{}",
        out["status"]
    );
}

#[test]
fn undo_conflicts() {
    // Lo stash non c'è più (un pop, che non entra nel diario): l'undo del push è rifiutato.
    let (r, mut s) = opened();
    run(&r, &mut s, "stash.push", json!({}));
    run(&r, &mut s, "stash.pop", json!({}));
    let status = run(&r, &mut s, "repo.status", json!({}));
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs[0].contains("non è più nell'elenco")),
        "{err}"
    );
    assert_eq!(run(&r, &mut s, "repo.status", json!({})), status);
    assert_eq!(run(&r, &mut s, "edit.history", json!({}))["can_undo"], true);

    // HEAD spostata da fuori dopo il push (un commit fatto senza passare dal diario).
    let (r, mut s) = opened();
    run(&r, &mut s, "stash.push", json!({}));
    let repo = s.repo_mut().unwrap();
    repo.stage(&[RepoPath::from("note.txt")]).unwrap();
    repo.commit(&crate::CommitRequest::new("da fuori")).unwrap();
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs[0].starts_with("HEAD")),
        "{err}"
    );
    assert_eq!(stash_count(&r, &mut s), 1);

    // Undo di un drop con lo stash di nuovo nell'elenco (rimesso a mano).
    let (r, mut s) = opened();
    let id = run(&r, &mut s, "stash.push", json!({}))["stash"]["id"].clone();
    run(
        &r,
        &mut s,
        "stash.drop",
        json!({ "confirm": true, "expected": id }),
    );
    let oid: project_domain::Oid = id.as_str().unwrap().parse().unwrap();
    s.repo_mut()
        .unwrap()
        .stash_store(&oid, "On main: a mano")
        .unwrap();
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs[0].contains("già nell'elenco")),
        "{err}"
    );
}

#[test]
fn log_with_the_stash_bases() {
    let mut repo = MemoryBackend::sample();
    let stash = repo.stash_push(&StashPush::default()).unwrap();
    // `main` torna indietro: la base dello stash non è più su nessun branch.
    let parent = repo.show_commit("HEAD").unwrap().commit.parents[0];
    repo.set_branch("main", parent);
    let (r, mut s) = opened_with(repo);
    let ids = |out: &Value| -> Vec<String> {
        out["commits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let base = stash.base.to_string();
    let all = run(&r, &mut s, "repo.log", json!({ "all": true }));
    assert!(!ids(&all).contains(&base));
    let with = run(
        &r,
        &mut s,
        "repo.log",
        json!({ "all": true, "stashes": true }),
    );
    assert!(ids(&with).contains(&base));
    assert!(!ids(&with).contains(&stash.id.to_string()), "lo stash no");
    let err = r
        .run("repo.log", &mut s, json!({ "stashes": true }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
}

#[test]
fn undo_of_a_keep_index_push_and_pop_after_it() {
    // `docs/guida.md` (`A.`) resta in stage con `keep_index`: ha già la versione dello stash
    // e non blocca né l'undo né il pop.
    let (r, mut s) = opened();
    let before = run(&r, &mut s, "repo.status", json!({}));
    let out = run(
        &r,
        &mut s,
        "stash.push",
        json!({ "keep_index": true, "paths": ["docs", "src"] }),
    );
    assert!(
        codes(&out["status"]).contains(&code("docs/guida.md", "Added", "Unmodified")),
        "{}",
        out["status"]
    );
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["status"], before);
    run(&r, &mut s, "edit.redo", json!({}));
    let out = run(&r, &mut s, "stash.pop", json!({ "restore_index": true }));
    assert_eq!(out["dropped"], true);
    assert_eq!(out["status"], before);
    // Un file `MM` rimasto con `keep_index` (worktree diverso da quello dello stash) blocca.
    let (r, mut s) = opened();
    run(&r, &mut s, "stash.push", json!({ "keep_index": true }));
    let err = r.run("stash.pop", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`README.md`") && !m.contains("guida")),
        "{err}"
    );
}

#[test]
fn a_failed_drop_after_the_apply_is_not_an_error_of_the_pop() {
    let mut repo = MemoryBackend::sample();
    repo.stash_push(&StashPush::default()).unwrap();
    repo.fail_stash_drop(Some("drop non riuscito"));
    let (r, mut s) = opened_with(repo);
    let out = run(&r, &mut s, "stash.pop", json!({}));
    assert_eq!(out["dropped"], false);
    assert_eq!(out["conflicts"], json!([]));
    assert!(
        out["warnings"][0]
            .as_str()
            .unwrap()
            .contains("applicato ma non tolto dall'elenco"),
        "{out}"
    );
    assert_eq!(out["stashes"].as_array().unwrap().len(), 1);
    assert_eq!(out["status"]["entries"].as_array().unwrap().len(), 4);

    // L'undo di un push: errore che dice che l'apply è avvenuto, la voce resta.
    let mut repo = MemoryBackend::sample();
    repo.fail_stash_drop(Some("drop non riuscito"));
    let (r, mut s) = opened_with(repo);
    run(&r, &mut s, "stash.push", json!({}));
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        err.to_string().contains("ha applicato lo stash")
            && err.to_string().contains("non ripetere l'undo"),
        "{err}"
    );
    assert_eq!(run(&r, &mut s, "edit.history", json!({}))["can_undo"], true);
}

#[test]
fn stash_commands_need_an_open_repo() {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(MemoryOpener::sample()));
    for id in [
        "stash.list",
        "stash.push",
        "stash.apply",
        "stash.pop",
        "stash.drop",
    ] {
        let err = r.run(id, &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NoRepo), "{id}: {err}");
    }
}
