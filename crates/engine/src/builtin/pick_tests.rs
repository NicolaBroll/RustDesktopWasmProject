//! Test di `commit.cherry_pick`, `commit.revert`, `*.continue`, `*.abort` e `repo.operation`
//! con cherry-pick e revert sul `MemoryBackend` (con git vero: `crates/git/tests/cherry_pick.rs`).
//!
//! Il repo: `c1` (radice: `a.txt`, `b.txt`); `main` su `c2` (`a.txt` = «nostra»); `altro` su
//! `c3` da `c1` (`a.txt` = «loro»: conflitto con `main`); `feat` su `c4` da `c1`, di un altro
//! autore (aggiunge `n.txt`); `dopo` su `c5` da `c2` (`a.txt` = «dopo»: il revert di `c2` lì è
//! in conflitto); `fusione` su `c6`, commit di merge di `c1` e `c4` (aggiunge `n.txt`). HEAD su
//! `main`, worktree pulito.

use crate::{EngineError, MemoryBackend, MemoryOpener, Registry, Session};
use project_domain::{Change, ChangedFile, Commit, EntryKind, Oid, RepoPath, Signature};
use serde_json::{Value, json};
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn sig(name: &str, n: i64) -> Signature {
    Signature {
        name: name.into(),
        email: format!("{}@example.com", name.to_lowercase()),
        time: 1_700_000_000 + n * 60,
        offset_min: 0,
    }
}

fn blob(repo: &mut MemoryBackend, content: &str) -> Oid {
    let hash = content.bytes().fold(1_469_598_103_934_665_603_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1_099_511_628_211)
    });
    let id = oid(hash);
    repo.set_blob(id, content);
    id
}

/// Il commit `n` con genitori `parents` e i file `files` (percorso, contenuto prima se c'era,
/// contenuto dopo) rispetto al primo genitore.
fn commit(
    repo: &mut MemoryBackend,
    n: u64,
    parents: &[u64],
    author: &str,
    summary: &str,
    files: &[(&str, Option<&str>, &str)],
) {
    let changes = files
        .iter()
        .map(|(path, old, new)| {
            let old_oid = old.map(|c| blob(repo, c));
            let new_oid = blob(repo, new);
            ChangedFile {
                path: RepoPath::from(*path),
                change: if old.is_some() {
                    Change::Modified
                } else {
                    Change::Added
                },
                original: None,
                score: None,
                old_mode: if old.is_some() { 0o100644 } else { 0 },
                new_mode: 0o100644,
                old_oid: old_oid.unwrap_or(Oid::from_bytes([0; Oid::LEN])),
                new_oid,
            }
        })
        .collect();
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parents.iter().map(|p| oid(*p)).collect(),
        author: sig(author, n as i64),
        committer: sig(author, n as i64),
        summary: summary.into(),
        body: String::new(),
    });
    repo.set_changes(oid(n), changes);
}

fn repo() -> MemoryBackend {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    let base = [("a.txt", None, "base\n"), ("b.txt", None, "b\n")];
    commit(&mut repo, 1, &[], "Ada", "base", &base);
    let nostra = [("a.txt", Some("base\n"), "nostra\n")];
    commit(&mut repo, 2, &[1], "Ada", "nostra", &nostra);
    let loro = [("a.txt", Some("base\n"), "loro\n")];
    commit(&mut repo, 3, &[1], "Ada", "loro", &loro);
    let nuovo = [("n.txt", None, "nuovo\n")];
    commit(&mut repo, 4, &[1], "Bea", "nuovo", &nuovo);
    let dopo = [("a.txt", Some("nostra\n"), "dopo\n")];
    commit(&mut repo, 5, &[2], "Ada", "dopo", &dopo);
    commit(&mut repo, 6, &[1, 4], "Ada", "Merge branch 'feat'", &nuovo);
    repo.set_branch("main", oid(2));
    repo.set_branch("altro", oid(3));
    repo.set_branch("feat", oid(4));
    repo.set_branch("dopo", oid(5));
    repo.set_branch("fusione", oid(6));
    repo.set_head_branch("main");
    repo
}

fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::default();
    opener.insert("r", repo);
    let mut s = Session::new(Arc::new(opener));
    let r = Registry::builtin();
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
    (r, s)
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn main_at(r: &Registry, s: &mut Session) -> String {
    run(r, s, "repo.status", json!({}))["head"]["Branch"]["target"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn status(r: &Registry, s: &mut Session) -> Value {
    run(r, s, "repo.status", json!({}))
}

#[test]
fn pick_commands_write() {
    let r = Registry::builtin();
    for id in [
        "commit.cherry_pick",
        "commit.revert",
        "cherry_pick.continue",
        "cherry_pick.abort",
        "revert.continue",
        "revert.abort",
    ] {
        assert!(r.get(id).unwrap().writes, "{id}");
    }
}

#[test]
fn cherry_pick_keeps_the_author_and_undo_redo() {
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "feat" }));
    assert_eq!(out["in_progress"], false);
    assert_eq!(out["commit"], oid(4).to_string());
    assert_eq!(out["before"], oid(2).to_string());
    assert_eq!(out["backup"], "refs/project/backup/cherry-pick-1");
    let after = out["after"].as_str().unwrap().to_owned();
    let show = run(&r, &mut s, "commit.show", json!({ "id": after }));
    assert_eq!(show["commit"]["summary"], "nuovo");
    assert_eq!(show["commit"]["author"]["name"], "Bea");
    assert_eq!(show["commit"]["committer"]["name"], "Ada");
    assert_eq!(show["commit"]["parents"], json!([oid(2).to_string()]));
    assert_eq!(show["files"][0]["path"], "n.txt");
    assert_eq!(status(&r, &mut s)["entries"], json!([]));
    assert_eq!(
        out["next_undo"],
        format!(
            "Cherry-pick di feat su `main` (da {} a {})",
            &oid(2).to_string()[..7],
            &after[..7]
        )
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
    assert_eq!(status(&r, &mut s)["entries"], json!([]));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(main_at(&r, &mut s), after);

    // Di nuovo: già applicato, niente cambia e niente resta in corso.
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "feat" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("non cambierebbe niente"), "{err}");
    assert_eq!(main_at(&r, &mut s), after);
    assert_eq!(status(&r, &mut s)["operation"], Value::Null);
}

#[test]
fn revert_writes_the_message_like_git() {
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "commit.revert", json!({ "rev": "main" }));
    assert_eq!(out["backup"], "refs/project/backup/revert-1");
    let show = run(&r, &mut s, "commit.show", json!({ "id": out["after"] }));
    assert_eq!(show["commit"]["summary"], "Revert \"nostra\"");
    assert_eq!(
        show["commit"]["body"],
        format!("This reverts commit {}.", oid(2))
    );
    assert_eq!(show["commit"]["author"]["name"], "Ada");
    assert_eq!(show["files"][0]["path"], "a.txt");
    assert!(
        out["next_undo"]
            .as_str()
            .unwrap()
            .starts_with("Revert di main su `main`")
    );
    // Il revert del revert lo riapplica; un secondo revert dello stesso commit è vuoto.
    let err = r
        .run(
            "commit.revert",
            &mut s,
            json!({ "rev": oid(2).to_string() }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("non cambierebbe niente"), "{err}");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
}

#[test]
fn conflict_stays_in_progress_then_continue_keeps_the_author() {
    let mut base = repo();
    // `altro` di un altro autore.
    let loro = [("a.txt", Some("base\n"), "loro\n")];
    commit(&mut base, 3, &[1], "Bea", "loro", &loro);
    let (r, mut s) = opened_with(base);
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "altro" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["after"], oid(2).to_string());
    assert_eq!(out["can_undo"], false);
    assert_eq!(out["status"]["operation"], "CherryPick");

    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "CherryPick");
    assert_eq!(op["incoming"][0]["id"], oid(3).to_string());
    assert_eq!(op["message"], "loro");
    assert_eq!(op["continue"], "cherry_pick.continue");
    assert_eq!(op["abort"], "cherry_pick.abort");

    // Durante l'operazione: undo, i comandi di un altro tipo e il continue con conflitti no.
    for (id, params, text) in [
        ("edit.undo", json!({}), ""),
        ("commit.revert", json!({ "rev": "feat" }), "cherry-pick"),
        ("merge.continue", json!({}), "non un merge"),
        ("revert.continue", json!({}), "non un revert"),
        ("revert.abort", json!({ "confirm": true }), "non un revert"),
        ("cherry_pick.continue", json!({}), "`a.txt`"),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
        assert!(err.to_string().contains(text), "{id}: {err}");
    }

    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    // `commit.create` non conclude un cherry-pick: solo `cherry_pick.continue`.
    let err = r
        .run("commit.create", &mut s, json!({ "message": "x" }))
        .unwrap_err();
    assert!(err.to_string().contains("cherry-pick"), "{err}");
    let out = run(&r, &mut s, "cherry_pick.continue", json!({}));
    assert_eq!(out["message"], "loro");
    assert_eq!(out["commit"]["author"]["name"], "Bea");
    assert_eq!(out["commit"]["parents"], json!([oid(2).to_string()]));
    assert_eq!(out["status"]["operation"], Value::Null);
    assert!(
        out["next_undo"]
            .as_str()
            .unwrap()
            .starts_with("Cherry-pick di altro su `main`")
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
    assert_eq!(status(&r, &mut s)["entries"], json!([]));
}

#[test]
fn revert_conflict_and_abort_with_backup() {
    let mut base = repo();
    base.set_branch("main", oid(5));
    let (r, mut s) = opened_with(base);
    let err = r
        .run("revert.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(err.to_string().contains("non c'è un revert"), "{err}");

    let out = run(
        &r,
        &mut s,
        "commit.revert",
        json!({ "rev": oid(2).to_string() }),
    );
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Revert");
    assert_eq!(op["continue"], "revert.continue");
    assert_eq!(op["abort"], "revert.abort");
    assert_eq!(
        op["message"],
        format!("Revert \"nostra\"\n\nThis reverts commit {}.", oid(2))
    );

    let err = r.run("revert.abort", &mut s, json!({})).unwrap_err();
    match err {
        EngineError::ConfirmRequired { reason, .. } => {
            assert!(reason.contains("`a.txt`"), "{reason}");
            assert!(reason.contains("revert-abort"), "{reason}");
        }
        other => panic!("{other}"),
    }
    assert_eq!(status(&r, &mut s)["operation"], "Revert");
    let out = run(&r, &mut s, "revert.abort", json!({ "confirm": true }));
    assert_eq!(out["backup"], "refs/project/backup/revert-abort-1");
    assert_eq!(out["files"], json!(["a.txt"]));
    assert_eq!(out["status"]["operation"], Value::Null);
    assert_eq!(out["status"]["entries"], json!([]));
    assert_eq!(out["can_undo"], false);
    assert_eq!(main_at(&r, &mut s), oid(5).to_string());
    let show = run(&r, &mut s, "commit.show", json!({ "id": out["backup"] }));
    assert_eq!(show["commit"]["parents"].as_array().unwrap().len(), 2);

    // Un cherry-pick fermo si annulla allo stesso modo.
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "altro" }));
    assert_eq!(out["in_progress"], true);
    let out = run(&r, &mut s, "cherry_pick.abort", json!({ "confirm": true }));
    assert_eq!(out["backup"], "refs/project/backup/cherry-pick-abort-1");
    assert_eq!(out["status"]["operation"], Value::Null);
}

#[test]
fn merge_commit_needs_mainline() {
    let (r, mut s) = opened_with(repo());
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "fusione" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    assert!(err.to_string().contains("mainline"), "{err}");
    for params in [
        json!({ "rev": "fusione", "mainline": 0 }),
        json!({ "rev": "fusione", "mainline": 3 }),
        json!({ "rev": "feat", "mainline": 1 }),
    ] {
        let err = r
            .run("commit.cherry_pick", &mut s, params.clone())
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
    let out = run(
        &r,
        &mut s,
        "commit.cherry_pick",
        json!({ "rev": "fusione", "mainline": 1 }),
    );
    assert_eq!(out["mainline"], 1);
    let show = run(&r, &mut s, "commit.show", json!({ "id": out["after"] }));
    assert_eq!(show["commit"]["parents"], json!([oid(2).to_string()]));
    assert_eq!(show["files"][0]["path"], "n.txt");
    // Il revert del merge rispetto al primo genitore: il messaggio di git.
    let out = run(
        &r,
        &mut s,
        "commit.revert",
        json!({ "rev": "fusione", "mainline": 1 }),
    );
    let show = run(&r, &mut s, "commit.show", json!({ "id": out["after"] }));
    assert_eq!(
        show["commit"]["body"],
        format!(
            "This reverts commit {}, reversing\nchanges made to {}.",
            oid(6),
            oid(1)
        )
    );
}

#[test]
fn refusals() {
    let (r, mut s) = opened_with(repo());
    for params in [
        json!({ "rev": "" }),
        json!({ "rev": "feat", "no_commit": true }),
    ] {
        for id in ["commit.cherry_pick", "commit.revert"] {
            let err = r.run(id, &mut s, params.clone()).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{id} {params}: {err}"
            );
        }
    }
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "nessuno" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    let err = r
        .run("cherry_pick.continue", &mut s, json!({}))
        .unwrap_err();
    assert!(err.to_string().contains("non c'è un cherry-pick"), "{err}");

    // HEAD staccata.
    let mut detached = repo();
    detached.detach_head(oid(2));
    let (r, mut s) = opened_with(detached);
    for id in ["commit.cherry_pick", "commit.revert"] {
        let err = r.run(id, &mut s, json!({ "rev": "feat" })).unwrap_err();
        assert!(err.to_string().contains("HEAD è staccata"), "{id}: {err}");
    }

    // Qualunque modifica ai file tracciati, anche a file che il commit non tocca.
    let mut dirty = repo();
    let (mode, b) = (0o100644, blob(&mut dirty, "b\n"));
    dirty.set_entries(vec![project_domain::StatusEntry {
        path: RepoPath::from("b.txt"),
        index: Change::Unmodified,
        worktree: Change::Modified,
        kind: EntryKind::Ordinary(project_domain::TrackedInfo {
            submodule: None,
            mode_head: mode,
            mode_index: mode,
            mode_worktree: mode,
            oid_head: b,
            oid_index: b,
        }),
    }]);
    dirty.set_worktree_file("b.txt", "cambiato\n");
    let (r, mut s) = opened_with(dirty);
    for id in ["commit.cherry_pick", "commit.revert"] {
        let err = r.run(id, &mut s, json!({ "rev": "feat" })).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
        assert!(err.to_string().contains("`b.txt`"), "{id}: {err}");
        assert!(err.to_string().contains("stash.push"), "{id}: {err}");
    }
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
}

#[test]
fn continue_resolved_as_head_points_to_abort() {
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "altro" }));
    assert_eq!(out["in_progress"], true);
    let shown = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    let digest = shown["digest"].clone();
    // Risolto tenendo la versione di HEAD: niente da committare.
    run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": digest, "content": "nostra\n" }),
    );
    let err = r
        .run("cherry_pick.continue", &mut s, json!({}))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("cherry_pick.abort"), "{err}");
    assert_eq!(status(&r, &mut s)["operation"], "CherryPick");
}
