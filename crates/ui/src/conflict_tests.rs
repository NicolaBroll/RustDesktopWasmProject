use super::*;
use crate::merge::{MergeBox, OPERATION};
use project_domain::{Change, ChangedFile, Commit, Oid, Signature};
use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn sig(n: u64) -> Signature {
    Signature {
        name: "Ada".into(),
        email: "ada@example.com".into(),
        time: 1_700_000_000 + i64::try_from(n).unwrap() * 60,
        offset_min: 0,
    }
}

/// Il commit `n` con genitore `parent` che porta `a.txt` dal blob `old` al blob `new`, il cui
/// contenuto è `text`.
fn commit(repo: &mut MemoryBackend, n: u64, parent: Option<u64>, text: &str) {
    let old = parent.map(|p| oid(100 + p));
    repo.set_blob(oid(100 + n), text.to_owned());
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parent.map(oid).into_iter().collect(),
        author: sig(n),
        committer: sig(n),
        summary: format!("c{n}"),
        body: String::new(),
    });
    repo.set_changes(
        oid(n),
        vec![ChangedFile {
            path: RepoPath::from("a.txt"),
            change: if old.is_some() {
                Change::Modified
            } else {
                Change::Added
            },
            original: None,
            score: None,
            old_mode: if old.is_some() { 0o100644 } else { 0 },
            new_mode: 0o100644,
            old_oid: old.unwrap_or(Oid::from_bytes([0; Oid::LEN])),
            new_oid: oid(100 + n),
        }],
    );
}

/// `main` su `c2` e `altro` su `c3`, tutti e due da `c1`: `merge.start` di `altro` si ferma in
/// conflitto su `a.txt`.
fn merging() -> (Registry, Session) {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    commit(&mut repo, 1, None, "base\n");
    commit(&mut repo, 2, Some(1), "nostra\n");
    commit(&mut repo, 3, Some(1), "loro\n");
    repo.set_branch("main", oid(2));
    repo.set_branch("altro", oid(3));
    repo.set_head_branch("main");
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(opener));
    registry
        .run("repo.open", &mut session, json!({ "path": "r" }))
        .unwrap();
    let started = registry
        .run("merge.start", &mut session, json!({ "rev": "altro" }))
        .unwrap();
    assert_eq!(started["in_progress"], true, "{started}");
    (registry, session)
}

/// Apre l'editor su `path` e passa la risposta di `conflict.show`.
fn open(r: &Registry, s: &mut Session, b: &mut ConflictBox, path: &str) {
    let (params, seq) = b.open(RepoPath::from(path)).unwrap();
    b.receive_show(seq, r.run(SHOW, s, params).map_err(|e| e.to_string()));
}

/// Esegue `conflict.resolve` come l'executor.
fn send(r: &Registry, s: &mut Session, b: &mut ConflictBox, params: Value) -> Received {
    let result = r.run(RESOLVE, s, params);
    let kind = result.as_ref().err().map(ErrorKind::of);
    b.receive_resolve(result.map_err(|e| e.to_string()), kind)
}

fn loaded(b: &ConflictBox) -> &Loaded {
    b.editor().and_then(Editor::loaded).expect("versioni lette")
}

/// Un editor già letto con questo testo del worktree, senza motore.
fn with_text(text: &str) -> ConflictBox {
    let mut b = ConflictBox::default();
    let (_, seq) = b.open(RepoPath::from("f.txt")).unwrap();
    let version = |t: &str| json!({ "text": t, "binary": false, "size": t.len() });
    b.receive_show(
        seq,
        Ok(json!({
            "path": "f.txt", "kind": "BothModified",
            "base": version("b\n"), "ours": version("o\n"), "theirs": version("t\n"),
            "worktree": version(text), "marker_size": 7, "digest": "d",
        })),
    );
    b
}

const TWO: &str = "inizio\n<<<<<<< HEAD\no1\n=======\nt1\n>>>>>>> feature\nmezzo\n<<<<<<< HEAD\no2\n\
                   =======\nt2\n>>>>>>> feature\nfine\n";

#[test]
fn conflitto_risolto_dalla_ui_e_merge_pronto() {
    let (r, mut s) = merging();
    let mut b = ConflictBox::default();
    open(&r, &mut s, &mut b, "a.txt");
    let l = loaded(&b);
    assert_eq!(l.base, Column::Text(vec!["base".into()]));
    assert_eq!(l.ours, Column::Text(vec!["nostra".into()]));
    assert_eq!(l.theirs, Column::Text(vec!["loro".into()]));
    assert_eq!(l.ours_label.as_deref(), Some("HEAD"));
    let blocks = l.blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!((blocks[0].line, blocks[0].has_base), (1, false));
    // Lo stesso file non si rilegge (le modifiche restano).
    assert!(b.open(RepoPath::from("a.txt")).is_none());

    // "Entrambe", poi a mano una riga in più.
    assert!(b.choose(0, Choice::OursThenTheirs));
    assert_eq!(loaded(&b).result(), Some("nostra\nloro\n"));
    b.set_result("nostra\nloro\nfine\n".into());
    assert!(loaded(&b).blocks().is_empty());

    let (params, label) = b.resolve().expect("senza marcatori si manda subito");
    assert_eq!(params["content"], "nostra\nloro\nfine\n");
    assert_eq!(label, "Risoluzione del conflitto di a.txt");
    assert!(b.busy());
    let Received::Done { status, notice } = send(&r, &mut s, &mut b, params) else {
        panic!("risolto");
    };
    assert!(!status.has_conflicts());
    assert!(notice.contains("a.txt risolto"), "{notice}");
    assert!(b.editor().is_none());

    // Il riquadro si rilegge con lo stato nuovo: niente conflitti, "Continua" si attiva.
    let mut merge = MergeBox::default();
    assert!(merge.want_info(status.operation, 1));
    merge.receive_info(
        r.run(OPERATION, &mut s, json!({}))
            .map_err(|e| e.to_string()),
    );
    let info = merge.info().expect("merge in corso");
    assert!(info.ready);
    assert!(info.conflicts.is_empty());
}

#[test]
fn file_cambiato_dopo_la_lettura_si_rilegge() {
    let (r, mut s) = merging();
    let mut b = ConflictBox::default();
    open(&r, &mut s, &mut b, "a.txt");
    assert!(b.choose(0, Choice::Theirs));
    let (mut params, _) = b.resolve().unwrap();
    params["digest"] = json!("0".repeat(32));
    let Received::Failed { reread, .. } = send(&r, &mut s, &mut b, params) else {
        panic!("StaleDiff");
    };
    let (params, seq) = reread.expect("si rilegge il file");
    let editor = b.editor().unwrap();
    assert!(matches!(editor.load, Load::Loading));
    assert!(editor.error.as_deref().unwrap().contains("cambiato"));
    b.receive_show(seq, r.run(SHOW, &mut s, params).map_err(|e| e.to_string()));
    assert_eq!(loaded(&b).blocks().len(), 1);
}

#[test]
fn marcatori_rimasti_chiedono_conferma_in_due_tempi() {
    let mut b = with_text(TWO);
    assert_eq!(loaded(&b).blocks().len(), 2);
    assert_eq!(loaded(&b).theirs_label.as_deref(), Some("feature"));
    assert!(b.choose(1, Choice::Ours));
    assert_eq!(
        loaded(&b).result(),
        Some("inizio\n<<<<<<< HEAD\no1\n=======\nt1\n>>>>>>> feature\nmezzo\no2\nfine\n")
    );
    let blocks = loaded(&b).blocks();
    assert_eq!((blocks.len(), blocks[0].line), (1, 2));

    // Primo tempo: niente da mandare, si apre la conferma.
    assert!(b.resolve().is_none());
    assert_eq!(b.editor().unwrap().pending, Some(Pending::Markers(1)));
    // Rinunciare non manda niente.
    b.cancel();
    assert_eq!(b.editor().unwrap().pending, None);
    assert!(b.confirm().is_none());
    // Secondo tempo: si manda il testo con i marcatori.
    assert!(b.resolve().is_none());
    let (params, _) = b.confirm().expect("confermato");
    assert!(params["content"].as_str().unwrap().contains("<<<<<<< HEAD"));
    assert_eq!(params["digest"], "d");
    assert!(b.busy());
}

#[test]
fn testo_non_analizzabile_niente_pulsanti_per_blocco() {
    // Marcatori rotti a mano: il testo resta modificabile, ma senza blocchi.
    let mut b = with_text(TWO);
    b.set_result("<<<<<<< HEAD\nx\n".into());
    let l = loaded(&b);
    assert!(l.blocks().is_empty());
    assert!(l.parse_error().is_some());
    assert_eq!(l.result(), Some("<<<<<<< HEAD\nx\n"));
    assert!(!b.choose(0, Choice::Ours));
    assert!(!b.choose_all(Choice::Theirs));
    assert!(b.editor().unwrap().error.is_some());
    // Senza blocchi analizzati si manda senza conferma (l'avviso è sui marcatori trovati).
    assert!(b.resolve().is_some());

    // Il file del worktree già rotto: niente risultato modificabile, solo i lati interi.
    let mut b = with_text("<<<<<<< HEAD\nsolo nostra\n");
    let l = loaded(&b);
    assert!(l.result().is_none());
    assert!(l.blocks().is_empty());
    assert!(b.resolve().is_none());
    let (params, label) = b.use_side(Pick::Theirs).unwrap();
    assert_eq!(params["content"], "t\n");
    assert!(label.contains("loro"), "{label}");
}

#[test]
fn tutto_loro_e_base() {
    let mut b = with_text(
        "<<<<<<< ours\na\n||||||| base\nb\n=======\nc\n>>>>>>> theirs\nx\n<<<<<<< ours\nd\n\
         ||||||| base\ne\n=======\nf\n>>>>>>> theirs\n",
    );
    assert!(loaded(&b).blocks().iter().all(|b| b.has_base));
    assert!(b.choose(1, Choice::Base));
    assert!(b.choose_all(Choice::Theirs));
    assert_eq!(loaded(&b).result(), Some("c\nx\ne\n"));
}

#[test]
fn eliminazione_chiede_conferma() {
    let mut b = ConflictBox::default();
    let (_, seq) = b.open(RepoPath::from("f.txt")).unwrap();
    b.receive_show(
        seq,
        Ok(json!({
            "path": "f.txt", "kind": "DeletedByUs", "base": null, "ours": null,
            "theirs": { "text": "t\n", "binary": false, "size": 2 },
            "worktree": null, "marker_size": 7, "digest": "d",
        })),
    );
    let l = loaded(&b);
    assert!(l.result().is_none() && l.can_delete());
    assert_eq!(l.ours, Column::Missing);
    assert!(b.use_side(Pick::Ours).is_none());
    b.delete();
    assert_eq!(b.editor().unwrap().pending, Some(Pending::Delete));
    let (params, _) = b.confirm().unwrap();
    assert_eq!(
        params,
        json!({ "path": "f.txt", "digest": "d", "delete": true })
    );
}

#[test]
fn risposte_vecchie_scartate_e_chiusura_col_nuovo_stato() {
    let mut b = ConflictBox::default();
    let (_, old) = b.open(RepoPath::from("a.txt")).unwrap();
    let (_, _) = b.restart().unwrap();
    b.receive_show(old, Err("vecchia".into()));
    assert!(matches!(b.editor().unwrap().load, Load::Loading));

    // Risolto altrove (stage dall'area delle modifiche): l'editor si chiude.
    let (r, mut s) = merging();
    let mut b = ConflictBox::default();
    open(&r, &mut s, &mut b, "a.txt");
    let status: Status =
        serde_json::from_value(r.run("repo.status", &mut s, json!({})).unwrap()).unwrap();
    b.follow_status(&status);
    assert!(b.editor().is_some());
    r.run("index.stage", &mut s, json!({ "paths": ["a.txt"] }))
        .unwrap();
    let status: Status =
        serde_json::from_value(r.run("repo.status", &mut s, json!({})).unwrap()).unwrap();
    b.follow_status(&status);
    assert!(b.editor().is_none());
}
