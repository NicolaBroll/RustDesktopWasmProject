//! Test di integrazione di `CliBackend::apply_lines` e dei comandi `index.stage_lines` e
//! `index.unstage_lines` su repo veri.
//!
//! Il contenuto dell'indice si legge con `git cat-file blob :<percorso>`; il worktree non deve
//! cambiare mai, nemmeno nella data di modifica dei file.

mod common;

use common::Repo;
use project_domain::{FileDiff, LineKind, LineRef, PatchDirection, RepoPath};
use project_engine::{
    DiffSpec, EngineError, GitBackend, LineSelection, Registry, Session, file_digest,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

#[cfg(unix)]
fn os(path: &[u8]) -> &OsStr {
    use std::os::unix::ffi::OsStrExt;
    OsStr::from_bytes(path)
}

#[cfg(not(unix))]
fn os(path: &[u8]) -> &OsStr {
    OsStr::new(std::str::from_utf8(path).unwrap())
}

/// Ogni file del worktree (fuori da `.git`): contenuto e data di modifica.
type Snapshot = BTreeMap<PathBuf, (Vec<u8>, SystemTime)>;

fn snapshot(repo: &Repo) -> Snapshot {
    fn walk(dir: &Path, root: &Path, out: &mut Snapshot) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.file_name() == Some(OsStr::new(".git")) {
                continue;
            }
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(
                    rel,
                    (std::fs::read(&path).unwrap(), meta.modified().unwrap()),
                );
            }
        }
    }
    let mut out = Snapshot::new();
    walk(repo.path(), repo.path(), &mut out);
    out
}

/// Il contenuto di `path` nell'indice (`git cat-file blob :<path>`); `None` se non c'è.
fn index(repo: &Repo, path: &[u8]) -> Option<Vec<u8>> {
    let mut spec = b":".to_vec();
    spec.extend_from_slice(path);
    let out = std::process::Command::new("git")
        .envs(common::GIT_ENV.iter().copied())
        .arg("-C")
        .arg(repo.path())
        .args(["cat-file", "blob"])
        .arg(os(&spec))
        .output()
        .unwrap();
    out.status.success().then_some(out.stdout)
}

fn index_str(repo: &Repo, path: &str) -> Option<String> {
    index(repo, path.as_bytes()).map(|b| String::from_utf8(b).unwrap())
}

/// L'indice intero (`git ls-files -s`), per verificare che non sia cambiato.
fn listing(repo: &Repo) -> String {
    repo.git(&["ls-files", "-s"])
}

fn backups(repo: &Repo) -> Vec<String> {
    let out = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/project/backup/",
    ]);
    out.lines().map(str::to_owned).collect()
}

fn numbered(n: usize, changed: &[(usize, &str)]) -> String {
    (1..=n)
        .map(|i| match changed.iter().find(|(j, _)| *j == i) {
            Some((_, text)) => format!("{text}\n"),
            None => format!("riga {i}\n"),
        })
        .collect()
}

/// Il file `path` del diff del worktree (stage) o dell'indice (unstage), come lo vede la UI.
fn seen(repo: &Repo, path: &[u8], direction: PatchDirection) -> FileDiff {
    let path = Some(RepoPath::from_bytes(path));
    let spec = match direction {
        PatchDirection::Stage => DiffSpec::Worktree { path },
        PatchDirection::Unstage => DiffSpec::Index { path },
    };
    let mut diff = repo.open().diff(&spec).unwrap();
    assert_eq!(diff.files.len(), 1, "un file nel diff: {diff:?}");
    diff.files.remove(0)
}

/// Il riferimento della riga di `file` con quel prefisso (`'-'` o `'+'`) e contenuto.
fn line(file: &FileDiff, prefix: char, content: &[u8]) -> LineRef {
    let kind = if prefix == '-' {
        LineKind::Removed
    } else {
        LineKind::Added
    };
    let mut found = file
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == kind && l.content.as_bytes() == content);
    let l = found.next().expect("riga presente nel diff");
    assert!(found.next().is_none(), "riga unica");
    l.line_ref().unwrap()
}

/// La selezione delle righe `picks` di `path`, con il digest del diff appena letto.
fn select(
    repo: &Repo,
    path: &[u8],
    direction: PatchDirection,
    picks: &[(char, &[u8])],
) -> LineSelection {
    let file = seen(repo, path, direction);
    LineSelection {
        path: RepoPath::from_bytes(path),
        lines: picks.iter().map(|(p, c)| line(&file, *p, c)).collect(),
        digest: file_digest(&file),
        direction,
    }
}

/// Applica la selezione con il backend e verifica che il worktree non sia cambiato.
fn apply(repo: &Repo, selection: &LineSelection) -> Result<Option<String>, EngineError> {
    let before = snapshot(repo);
    let result = repo.open().apply_lines(selection);
    assert_eq!(snapshot(repo), before, "il worktree non cambia");
    result
}

fn stage(repo: &Repo, path: &str, picks: &[(char, &str)]) -> Option<String> {
    let picks: Vec<(char, &[u8])> = picks.iter().map(|(p, c)| (*p, c.as_bytes())).collect();
    let sel = select(repo, path.as_bytes(), PatchDirection::Stage, &picks);
    apply(repo, &sel).unwrap()
}

fn unstage(repo: &Repo, path: &str, picks: &[(char, &str)]) -> Option<String> {
    let picks: Vec<(char, &[u8])> = picks.iter().map(|(p, c)| (*p, c.as_bytes())).collect();
    let sel = select(repo, path.as_bytes(), PatchDirection::Unstage, &picks);
    apply(repo, &sel).unwrap()
}

/// Esito non riuscito: l'errore, con indice, ref di backup e worktree invariati.
fn refused(repo: &Repo, selection: &LineSelection) -> EngineError {
    let (index_before, backups_before) = (listing(repo), backups(repo));
    let err = apply(repo, selection).unwrap_err();
    assert_eq!(listing(repo), index_before, "l'indice non cambia: {err}");
    assert_eq!(backups(repo), backups_before, "nessun backup: {err}");
    err
}

/// `(X, Y)` di `path` nello stato.
fn letters(repo: &Repo, path: &str) -> (char, char) {
    let status = repo.open().status().unwrap();
    let e = status
        .entries
        .iter()
        .find(|e| e.path == RepoPath::from(path))
        .expect("voce nello stato");
    (e.index.letter(), e.worktree.letter())
}

#[test]
fn hunk_righe_sparse_e_unstage_di_parte() {
    let repo = Repo::new();
    repo.commit_file("f.txt", &numbered(30, &[]), "base");
    let changed = numbered(
        30,
        &[(2, "DUE"), (3, "TRE"), (15, "QUINDICI"), (29, "XXIX")],
    );
    repo.write("f.txt", &changed);
    assert_eq!(seen(&repo, b"f.txt", PatchDirection::Stage).hunks.len(), 3);

    // Un hunk intero: quello in mezzo.
    let file = seen(&repo, b"f.txt", PatchDirection::Stage);
    let sel = LineSelection {
        path: RepoPath::from("f.txt"),
        lines: file.hunks[1].line_refs(),
        digest: file_digest(&file),
        direction: PatchDirection::Stage,
    };
    assert_eq!(apply(&repo, &sel).unwrap(), None);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(30, &[(15, "QUINDICI")])
    );
    assert_eq!(letters(&repo, "f.txt"), ('M', 'M'));

    // Righe sparse in due hunk: la versione in stage (`MM`) cambia, prima un backup.
    let backup = stage(
        &repo,
        "f.txt",
        &[
            ('-', "riga 2"),
            ('+', "DUE"),
            ('+', "XXIX"),
            ('-', "riga 29"),
        ],
    )
    .expect("backup della versione in stage");
    assert_eq!(
        repo.git(&["show", &format!("{backup}:f.txt")]),
        numbered(30, &[(15, "QUINDICI")]).trim_end()
    );
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(30, &[(2, "DUE"), (15, "QUINDICI"), (29, "XXIX")])
    );

    // Unstage di una parte, dal diff dell'indice.
    let backup = unstage(&repo, "f.txt", &[('-', "riga 15"), ('+', "QUINDICI")]);
    assert!(
        backup.is_some(),
        "anche qui la versione in stage era solo nell'indice"
    );
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        numbered(30, &[(2, "DUE"), (29, "XXIX")])
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("f.txt")).unwrap(),
        changed
    );
    assert_eq!(backups(&repo).len(), 2);
}

#[test]
fn nessun_backup_se_la_versione_in_stage_non_e_a_rischio() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\nc\n", "base");
    repo.write("f.txt", "a\nB\nc\nd\n");
    // `.M`: l'indice è uguale a HEAD.
    assert_eq!(stage(&repo, "f.txt", &[('+', "d")]), None);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nb\nc\nd\n");
    // Tutto in stage (`M.`): l'indice è uguale al worktree.
    repo.stage(&["f.txt"]);
    assert_eq!(unstage(&repo, "f.txt", &[('+', "d")]), None);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nB\nc\n");
    assert!(backups(&repo).is_empty());
}

#[test]
fn file_nuovo_intent_to_add() {
    let repo = Repo::new();
    repo.commit_file("base.txt", "x\n", "base");
    repo.write("nuovo.txt", "uno\ndue\ntre\n");
    repo.git(&["add", "-N", "nuovo.txt"]);
    assert_eq!(letters(&repo, "nuovo.txt"), ('.', 'A'));
    assert_eq!(
        stage(&repo, "nuovo.txt", &[('+', "uno"), ('+', "tre")]),
        None
    );
    assert_eq!(index_str(&repo, "nuovo.txt").unwrap(), "uno\ntre\n");
    assert_eq!(letters(&repo, "nuovo.txt"), ('A', 'M'));
    // E l'unstage di tutte le righe di un file nuovo lo toglie dall'indice.
    let backup = unstage(&repo, "nuovo.txt", &[('+', "uno"), ('+', "tre")]);
    assert!(
        backup.is_some(),
        "`AM`: la versione in stage era solo nell'indice"
    );
    assert_eq!(index(&repo, b"nuovo.txt"), None);
    assert_eq!(letters(&repo, "nuovo.txt"), ('?', '?'));
}

#[test]
fn file_eliminato_in_parte_e_poi_tutto() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\nc\n", "base");
    std::fs::remove_file(repo.path().join("f.txt")).unwrap();
    assert_eq!(stage(&repo, "f.txt", &[('-', "b")]), None);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nc\n");
    assert_eq!(letters(&repo, "f.txt"), ('M', 'D'));
    // Le righe rimaste: il file esce dall'indice. La versione in stage (`MD`) va nel backup.
    let backup = stage(&repo, "f.txt", &[('-', "a"), ('-', "c")]).unwrap();
    assert_eq!(index(&repo, b"f.txt"), None);
    assert_eq!(letters(&repo, "f.txt"), ('D', '.'));
    assert_eq!(repo.git(&["show", &format!("{backup}:f.txt")]), "a\nc");
    // L'unstage di una riga di un file eliminato in stage lo rimette nell'indice.
    assert_eq!(unstage(&repo, "f.txt", &[('-', "b")]), None);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "b\n");
}

#[test]
fn fine_file_senza_a_capo() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb", "base");
    repo.write("f.txt", "a\nb\nc\nd");
    assert_eq!(stage(&repo, "f.txt", &[('+', "c")]), None);
    assert_eq!(index(&repo, b"f.txt").unwrap(), b"a\nb\nc\n");
    assert!(
        stage(&repo, "f.txt", &[('+', "d")]).is_some(),
        "`MM`: backup"
    );
    assert_eq!(index(&repo, b"f.txt").unwrap(), b"a\nb\nc\nd");
}

#[test]
fn crlf_byte_per_byte() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\r\nb\r\nc\r\n", "base");
    repo.write("f.txt", "a\r\nB\r\nc\r\nd\r\n");
    assert_eq!(stage(&repo, "f.txt", &[('+', "d\r")]), None);
    assert_eq!(index(&repo, b"f.txt").unwrap(), b"a\r\nb\r\nc\r\nd\r\n");
    stage(&repo, "f.txt", &[('-', "b\r"), ('+', "B\r")]);
    assert_eq!(index(&repo, b"f.txt").unwrap(), b"a\r\nB\r\nc\r\nd\r\n");
}

#[cfg(unix)]
#[test]
fn nome_non_utf8_con_spazi() {
    let repo = Repo::new();
    let name: &[u8] = b"caff\xff con spazio.txt";
    std::fs::write(repo.path().join(os(name)), "a\nb\n").unwrap();
    repo.git(&["add", "-A"]);
    repo.commit("base");
    std::fs::write(repo.path().join(os(name)), "a\nB\nc\n").unwrap();
    let sel = select(&repo, name, PatchDirection::Stage, &[('+', b"c")]);
    assert_eq!(apply(&repo, &sel).unwrap(), None);
    assert_eq!(index(&repo, name).unwrap(), b"a\nb\nc\n");
}

#[test]
fn configurazione_dell_utente_neutralizzata() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\n", "base");
    repo.git(&["config", "apply.whitespace", "error"]);
    repo.git(&["config", "apply.ignoreWhitespace", "change"]);
    repo.git(&[
        "config",
        "core.whitespace",
        "trailing-space,space-before-tab",
    ]);
    repo.write("f.txt", "a\nb  \n \tc\n");
    stage(&repo, "f.txt", &[('+', "b  "), ('-', "b"), ('+', " \tc")]);
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nb  \n \tc\n");
}

#[test]
fn selezione_non_valida() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\nc\n", "base");
    repo.write("f.txt", "a\nB\nc\n");
    let file = seen(&repo, b"f.txt", PatchDirection::Stage);
    for lines in [
        vec![LineRef::New(1)], // contesto
        vec![LineRef::Old(9)], // inesistente
        vec![LineRef::Old(1)], // contesto, dal lato vecchio
        vec![LineRef::New(2), LineRef::New(7)],
        vec![],
    ] {
        let sel = LineSelection {
            path: RepoPath::from("f.txt"),
            lines,
            digest: file_digest(&file),
            direction: PatchDirection::Stage,
        };
        let err = refused(&repo, &sel);
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    }
    // Nessuna modifica in stage: niente da togliere.
    let sel = LineSelection {
        direction: PatchDirection::Unstage,
        path: RepoPath::from("f.txt"),
        lines: vec![LineRef::New(2)],
        digest: file_digest(&file),
    };
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
}

#[test]
fn file_cambiato_fra_diff_e_applicazione() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\nc\n", "base");
    repo.write("f.txt", "a\nB\nc\n");
    let sel = select(&repo, b"f.txt", PatchDirection::Stage, &[('+', b"B")]);

    // Stessi numeri di riga, contenuto diverso nel worktree.
    repo.write("f.txt", "a\nX\nc\n");
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");

    // Indice cambiato da fuori (stesso worktree di quando il diff è stato letto).
    repo.write("f.txt", "a\nB\nc\n");
    let sel = select(&repo, b"f.txt", PatchDirection::Stage, &[('+', b"B")]);
    repo.write("f.txt", "a\nb\nc\nd\n");
    repo.stage(&["f.txt"]);
    repo.write("f.txt", "a\nB\nc\n");
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nb\nc\nd\n");

    // Il file non ha più modifiche.
    let sel = select(&repo, b"f.txt", PatchDirection::Stage, &[('+', b"B")]);
    repo.stage(&["f.txt"]);
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
}

#[test]
fn binario_e_sottomodulo_rifiutati() {
    let repo = Repo::new();
    std::fs::write(repo.path().join("bin.dat"), b"a\0b\n").unwrap();
    repo.stage(&["bin.dat"]);
    let head = repo.commit("base");
    std::fs::write(repo.path().join("bin.dat"), b"a\0c\n").unwrap();
    let file = seen(&repo, b"bin.dat", PatchDirection::Stage);
    assert!(file.binary);
    let sel = LineSelection {
        path: RepoPath::from("bin.dat"),
        lines: vec![LineRef::New(1)],
        digest: file_digest(&file),
        direction: PatchDirection::Stage,
    };
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    // Un sottomodulo (gitlink) cambiato in stage.
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},sub"),
    ]);
    let second = repo.commit("sottomodulo");
    repo.git(&[
        "update-index",
        "--cacheinfo",
        &format!("160000,{second},sub"),
    ]);
    let file = seen(&repo, b"sub", PatchDirection::Unstage);
    assert!(file.is_submodule());
    let sel = LineSelection {
        path: RepoPath::from("sub"),
        lines: file.line_refs(),
        digest: file_digest(&file),
        direction: PatchDirection::Unstage,
    };
    let err = refused(&repo, &sel);
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
}

#[test]
fn indice_bloccato() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\n", "base");
    repo.write("f.txt", "a\nB\n");
    repo.stage(&["f.txt"]);
    repo.write("f.txt", "a\nB\nc\n");
    // `MM`: servirebbe un backup, ma con l'indice bloccato non si salva niente.
    let sel = select(&repo, b"f.txt", PatchDirection::Stage, &[('+', b"c")]);
    let lock = repo.path().join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    let err = refused(&repo, &sel);
    let EngineError::IndexLocked { lock: reported } = &err else {
        panic!("{err}");
    };
    assert_eq!(Path::new(reported), lock);
    assert!(lock.exists(), "il lock non si cancella mai");
    std::fs::remove_file(&lock).unwrap();
    assert!(apply(&repo, &sel).unwrap().is_some());
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nB\nc\n");
}

#[test]
fn unstage_di_una_rinomina_per_vecchio_percorso() {
    let repo = Repo::new();
    repo.commit_file("vecchio.txt", "uno\ndue\ntre\nquattro\n", "base");
    repo.git(&["mv", "vecchio.txt", "nuovo.txt"]);
    repo.write("nuovo.txt", "uno\ndue\ntre\nquattro\ncinque\n");
    repo.stage(&["nuovo.txt"]);
    let sel = select(
        &repo,
        b"vecchio.txt",
        PatchDirection::Unstage,
        &[('+', b"cinque")],
    );
    assert_eq!(apply(&repo, &sel).unwrap(), None);
    assert_eq!(
        index_str(&repo, "nuovo.txt").unwrap(),
        "uno\ndue\ntre\nquattro\n"
    );
    assert_eq!(index(&repo, b"vecchio.txt"), None);
}

fn run(registry: &Registry, session: &mut Session, id: &str, params: Value) -> Value {
    registry
        .run(id, session, params)
        .unwrap_or_else(|e| panic!("{id}: {e}"))
}

#[test]
fn comandi_sul_repo_vero() {
    let repo = Repo::new();
    repo.commit_file("f.txt", "a\nb\nc\n", "base");
    repo.write("f.txt", "a\nB\nc\nd\n");
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(Repo::opener()));
    run(
        &registry,
        &mut session,
        "repo.open",
        json!({ "path": repo.path_str() }),
    );
    let diff = run(
        &registry,
        &mut session,
        "diff.worktree",
        json!({ "path": "f.txt" }),
    );
    let file = &diff["files"][0];
    let digest = file["digest"].as_str().unwrap().to_owned();
    let parsed: FileDiff = serde_json::from_value(file.clone()).unwrap();
    assert_eq!(file_digest(&parsed), digest);

    let before = snapshot(&repo);
    let out = run(
        &registry,
        &mut session,
        "index.stage_lines",
        json!({ "path": "f.txt", "lines": [{ "new": 4 }], "digest": digest }),
    );
    assert_eq!(snapshot(&repo), before);
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["status"]["entries"][0]["index"], "Modified");
    assert_eq!(out["status"]["entries"][0]["worktree"], "Modified");
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nb\nc\nd\n");

    // Lo stesso digest ora è vecchio.
    let err = registry
        .run(
            "index.stage_lines",
            &mut session,
            json!({ "path": "f.txt", "lines": [{ "new": 2 }], "digest": digest }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");

    let diff = run(
        &registry,
        &mut session,
        "diff.index",
        json!({ "path": "f.txt" }),
    );
    let digest = diff["files"][0]["digest"].clone();
    let out = run(
        &registry,
        &mut session,
        "index.unstage_lines",
        json!({ "path": "f.txt", "lines": [{ "new": 4 }], "digest": digest }),
    );
    // `MM`: la versione in stage era solo nell'indice.
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(
        repo.git(&["show", &format!("{backup}:f.txt")]),
        "a\nb\nc\nd"
    );
    assert_eq!(index_str(&repo, "f.txt").unwrap(), "a\nb\nc\n");
    assert_eq!(out["status"]["entries"][0]["index"], "Unmodified");
}

/// `index.stage` (S18) di un file `intent-to-add`: `git status` dà modo e id zero per la
/// versione in stage, ma nell'indice c'è una voce; il ricontrollo dell'indice prima di scrivere
/// non deve scambiarla per una modifica concorrente.
#[test]
fn stage_intero_di_un_file_intent_to_add() {
    let repo = Repo::new();
    repo.commit_file("base.txt", "x\n", "base");
    repo.write("nuovo.txt", "uno\n");
    repo.git(&["add", "-N", "nuovo.txt"]);
    let mut backend = repo.open();
    assert_eq!(backend.stage(&[RepoPath::from("nuovo.txt")]).unwrap(), None);
    assert_eq!(index_str(&repo, "nuovo.txt").unwrap(), "uno\n");
    assert_eq!(letters(&repo, "nuovo.txt"), ('A', '.'));
}

/// Con `diff.context=0` nella configurazione il diff resta a tre righe di contesto
/// (`--unified=3` in `DIFF_ARGS`) e le righe vanno al loro posto nell'indice, anche in mezzo al
/// file e senza a capo finale (un hunk senza contesto `git apply` lo ancorerebbe in fondo).
#[test]
fn contesto_zero_nella_configurazione() {
    let repo = Repo::new();
    repo.git(&["config", "diff.context", "0"]);
    repo.commit_file("f.txt", "uno\ndue\ntre\nquattro\n", "base");
    repo.write("f.txt", "uno\ndue\nNUOVA\ntre\nquattro\n");
    let file = seen(&repo, b"f.txt", PatchDirection::Stage);
    assert_eq!(
        file.hunks[0].lines.len(),
        5,
        "contesto intorno all'inserimento"
    );
    assert_eq!(stage(&repo, "f.txt", &[('+', "NUOVA")]), None);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        "uno\ndue\nNUOVA\ntre\nquattro\n"
    );
    // E l'unstage della stessa riga, sempre con contesto zero.
    assert_eq!(unstage(&repo, "f.txt", &[('+', "NUOVA")]), None);
    assert_eq!(
        index_str(&repo, "f.txt").unwrap(),
        "uno\ndue\ntre\nquattro\n"
    );

    // Senza a capo finale le righe non si fondono.
    repo.commit_file("g.txt", "uno\ndue\ntre", "senza a capo");
    repo.write("g.txt", "uno\ndue\nNUOVA\ntre");
    assert_eq!(stage(&repo, "g.txt", &[('+', "NUOVA")]), None);
    assert_eq!(index(&repo, b"g.txt").unwrap(), b"uno\ndue\nNUOVA\ntre");
    // Due inserimenti lontani, uno solo scelto.
    repo.commit_file("h.txt", &numbered(20, &[]), "lungo");
    let mut changed = numbered(20, &[]).replace("riga 3\n", "riga 3\nPRIMA\n");
    changed = changed.replace("riga 17\n", "riga 17\nSECONDA\n");
    repo.write("h.txt", &changed);
    assert_eq!(stage(&repo, "h.txt", &[('+', "PRIMA")]), None);
    assert_eq!(
        index_str(&repo, "h.txt").unwrap(),
        numbered(20, &[]).replace("riga 3\n", "riga 3\nPRIMA\n")
    );
}

/// Le lettere di `git ls-files -v` per `path` (`H` normale, `S` skip-worktree, `h`
/// assume-unchanged).
fn tag(repo: &Repo, path: &str) -> String {
    repo.git(&["ls-files", "-v", "--", path])[..1].to_owned()
}

/// `update-index --index-info` e `restore --staged` ricreano la voce: i flag
/// `--skip-worktree` e `--assume-unchanged` vanno rimessi. Un file fuori dal cono di uno
/// sparse checkout (skip-worktree, assente dal worktree) con una modifica in stage non deve
/// sembrare eliminato dopo l'unstage di righe o del file.
#[test]
fn skip_worktree_e_assume_unchanged_restano() {
    let repo = Repo::new();
    repo.commit_file("d/f", "a\nb\nc\n", "base");
    repo.commit_file("u.txt", "a\nb\nc\n", "base u");
    repo.write("d/f", "a\nB\nc\nd\n");
    repo.write("u.txt", "a\nB\nc\nd\n");
    repo.stage(&["d/f", "u.txt"]);
    repo.git(&["update-index", "--skip-worktree", "d/f"]);
    repo.git(&["update-index", "--assume-unchanged", "u.txt"]);
    std::fs::remove_file(repo.path().join("d/f")).unwrap();
    assert_eq!(
        (tag(&repo, "d/f"), tag(&repo, "u.txt")),
        ("S".into(), "h".into())
    );
    assert_eq!(letters(&repo, "d/f"), ('M', '.'));

    // Unstage di una riga: le versioni in stage non sono visibili nel worktree, backup.
    assert!(unstage(&repo, "d/f", &[('+', "d")]).is_some());
    assert!(unstage(&repo, "u.txt", &[('+', "d")]).is_some());
    assert_eq!(index_str(&repo, "d/f").unwrap(), "a\nB\nc\n");
    assert_eq!(index_str(&repo, "u.txt").unwrap(), "a\nB\nc\n");
    assert_eq!(
        (tag(&repo, "d/f"), tag(&repo, "u.txt")),
        ("S".into(), "h".into())
    );
    assert_eq!(letters(&repo, "d/f"), ('M', '.'), "non sembra eliminato");

    // Anche `index.unstage` (S18, `restore --staged`).
    let mut backend = repo.open();
    backend
        .unstage(&[RepoPath::from("d"), RepoPath::from("u.txt")])
        .unwrap();
    assert_eq!(index_str(&repo, "d/f").unwrap(), "a\nb\nc\n");
    assert_eq!(
        (tag(&repo, "d/f"), tag(&repo, "u.txt")),
        ("S".into(), "h".into())
    );
    let status = repo.open().status().unwrap();
    assert!(
        !status
            .entries
            .iter()
            .any(|e| e.path == RepoPath::from("d/f")),
        "{status:?}"
    );
}
