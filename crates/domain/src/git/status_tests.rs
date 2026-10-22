use super::*;
use crate::DomainError;
use proptest::prelude::*;

// Output reale di git 2.43.0, generato da `testdata/status.sh` (`sh status.sh testdata`) in repo
// temporanei con `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1` e date fisse. Il comando
// è sempre
//
// ```sh
// git status --porcelain=v2 -z --branch --untracked-files=all
// ```
//
// cioè gli argomenti di [`STATUS_ARGS`], tranne dove indicato.

/// Branch `main` con upstream `origin/main` (+2 -1) e ogni tipo di modifica: in stage, nel
/// worktree, entrambe, aggiunti, cancellati, rinominati (anche con nome non UTF-8), cambio di
/// tipo e di modo, `git add -N`, sottomodulo modificato, non tracciati con nomi strani.
const MISTO: &[u8] = include_bytes!("testdata/status-misto.bin");
/// Merge con conflitti `UD`, `DU`, `AA`, `UU` su `main` senza upstream.
const CONFLITTO: &[u8] = include_bytes!("testdata/status-conflitto.bin");
/// HEAD staccata su `HEAD~1` con un file modificato.
const STACCATA: &[u8] = include_bytes!("testdata/status-staccata.bin");
/// Repo appena creato con `git init`: solo intestazioni, `branch.oid (initial)`.
const VUOTO: &[u8] = include_bytes!("testdata/status-vuoto.bin");
/// Repo senza commit con un file in stage e uno non tracciato.
const VUOTO_FILE: &[u8] = include_bytes!("testdata/status-vuoto-file.bin");
/// Upstream configurato ma senza il ref `refs/remotes/origin/main`: niente `branch.ab`.
const UPSTREAM_SPARITO: &[u8] = include_bytes!("testdata/status-upstream-sparito.bin");
/// Stesso comando con `--ignored` in più.
const IGNORATI: &[u8] = include_bytes!("testdata/status-ignorati.bin");
/// `git -c status.renames=copies status --porcelain=v2 -z --branch --untracked-files=all`.
const COPIE: &[u8] = include_bytes!("testdata/status-copie.bin");

fn oid(hex: &str) -> Oid {
    Oid::from_hex(hex.as_bytes()).expect("oid di test valido")
}

fn zero() -> Oid {
    Oid::from_bytes([0; 20])
}

fn parse(input: &[u8]) -> Status {
    parse_status(input).expect("output reale valido")
}

fn reason(err: DomainError) -> String {
    match err {
        DomainError::InvalidGitOutput { command, reason } => {
            assert_eq!(command, "status");
            reason
        }
        other => panic!("errore inatteso: {other:?}"),
    }
}

fn entry<'a>(status: &'a Status, path: &[u8]) -> &'a StatusEntry {
    status
        .entries
        .iter()
        .find(|e| e.path.as_bytes() == path)
        .unwrap_or_else(|| panic!("manca {}", String::from_utf8_lossy(path)))
}

fn xy(e: &StatusEntry) -> String {
    format!("{}{}", e.index.letter(), e.worktree.letter())
}

#[test]
fn misto_head_e_upstream() {
    let s = parse(MISTO);
    assert_eq!(
        s.head,
        Head::Branch {
            name: "main".into(),
            target: Some(oid("2b0d6ad3306129ed894debca4a22585244dc1880")),
        }
    );
    assert_eq!(
        s.upstream,
        Some(Upstream {
            name: "origin/main".into(),
            ahead: Some(2),
            behind: Some(1),
        })
    );
    assert_eq!(s.operation, None);
    assert!(!s.is_clean());
    assert!(!s.has_conflicts());
}

#[test]
fn misto_codici_xy_in_ordine() {
    let s = parse(MISTO);
    let got: Vec<(String, String)> = s
        .entries
        .iter()
        .map(|e| (xy(e), e.path.to_string()))
        .collect();
    let want: Vec<(String, String)> = [
        ("A.", "aggiunto.txt"),
        (".D", "cancellato-wt.txt"),
        ("D.", "cancellato.txt"),
        (".M", "con spazio.txt"),
        ("MM", "entrambi.txt"),
        (".M", "eseguibile.sh"),
        ("M.", "in-stage.txt"),
        (".A", "intento.txt"),
        (".M", "modificato.txt"),
        (".M", "modulo"),
        ("R.", "nuovo-nome.txt"),
        ("R.", "nuovo-\u{fffd}.txt"),
        (".T", "tipo.txt"),
        ("??", "a\ncapo.txt"),
        ("??", "cartella/interna/profondo.txt"),
        ("??", "non-utf8-\u{fffd}.txt"),
    ]
    .iter()
    .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
    .collect();
    assert_eq!(got, want);
}

#[test]
fn in_stage_e_nel_worktree() {
    let s = parse(MISTO);
    let staged: Vec<String> = s
        .entries
        .iter()
        .filter(|e| e.is_staged())
        .map(|e| e.path.to_string())
        .collect();
    assert_eq!(
        staged,
        [
            "aggiunto.txt",
            "cancellato.txt",
            "entrambi.txt",
            "in-stage.txt",
            "nuovo-nome.txt",
            "nuovo-\u{fffd}.txt",
        ]
    );
    let unstaged: Vec<String> = s
        .entries
        .iter()
        .filter(|e| e.is_unstaged())
        .map(|e| e.path.to_string())
        .collect();
    assert_eq!(
        unstaged,
        [
            "cancellato-wt.txt",
            "con spazio.txt",
            "entrambi.txt",
            "eseguibile.sh",
            "intento.txt",
            "modificato.txt",
            "modulo",
            "tipo.txt",
        ]
    );
}

#[test]
fn file_modificato_con_modi_e_id() {
    let s = parse(MISTO);
    let e = entry(&s, b"entrambi.txt");
    assert_eq!((e.index, e.worktree), (Change::Modified, Change::Modified));
    assert_eq!(
        e.kind,
        EntryKind::Ordinary(TrackedInfo {
            submodule: None,
            mode_head: 0o100644,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: oid("9a5cff1499090e2d008c8afb0e082cf9e2f9105e"),
            oid_index: oid("c06d79d030d595369b929b9c7046e9da56da9d81"),
        })
    );
    // Bit di esecuzione: cambia solo il modo nel worktree.
    let EntryKind::Ordinary(info) = &entry(&s, b"eseguibile.sh").kind else {
        panic!("voce ordinaria attesa");
    };
    assert_eq!((info.mode_index, info.mode_worktree), (0o100644, 0o100755));
    // Da file a link simbolico.
    let e = entry(&s, b"tipo.txt");
    assert_eq!(e.worktree, Change::TypeChanged);
    let EntryKind::Ordinary(info) = &e.kind else {
        panic!("voce ordinaria attesa");
    };
    assert_eq!(info.mode_worktree, 0o120000);
}

#[test]
fn aggiunto_e_cancellato_hanno_id_zero() {
    let s = parse(MISTO);
    let EntryKind::Ordinary(added) = &entry(&s, b"aggiunto.txt").kind else {
        panic!("voce ordinaria attesa");
    };
    assert_eq!((added.mode_head, added.oid_head), (0, zero()));
    let EntryKind::Ordinary(deleted) = &entry(&s, b"cancellato.txt").kind else {
        panic!("voce ordinaria attesa");
    };
    assert_eq!((deleted.mode_index, deleted.oid_index), (0, zero()));
    // `git add -N`: nell'indice c'è una voce vuota, quindi `.A`.
    let e = entry(&s, b"intento.txt");
    assert_eq!((e.index, e.worktree), (Change::Unmodified, Change::Added));
}

#[test]
fn rinomine_con_percorso_originale() {
    let s = parse(MISTO);
    let e = entry(&s, b"nuovo-nome.txt");
    assert_eq!(e.index, Change::Renamed);
    assert_eq!(e.original_path(), Some(&RepoPath::from("vecchio-nome.txt")));
    let EntryKind::Renamed { score, .. } = e.kind else {
        panic!("rinomina attesa");
    };
    assert_eq!(score, 100);
}

#[test]
fn percorsi_non_utf8_mantengono_i_byte() {
    let s = parse(MISTO);
    let e = entry(&s, b"nuovo-\xe9.txt");
    assert_eq!(
        e.original_path().map(RepoPath::as_bytes),
        Some(&b"vecchio-\xe9.txt"[..])
    );
    assert_eq!(e.path.to_str(), None);
    assert_eq!(e.path.to_string_lossy(), "nuovo-\u{fffd}.txt");
    let e = entry(&s, b"non-utf8-\xff.txt");
    assert!(e.is_untracked());
    // Spazi e a capo nel nome: con `-z` git non li mette tra virgolette.
    assert!(entry(&s, b"con spazio.txt").is_unstaged());
    assert!(entry(&s, b"a\ncapo.txt").is_untracked());
}

#[test]
fn non_tracciati_anche_in_cartelle_nuove() {
    let s = parse(MISTO);
    let e = entry(&s, b"cartella/interna/profondo.txt");
    assert_eq!(e.kind, EntryKind::Untracked);
    assert_eq!(
        (e.index, e.worktree),
        (Change::Untracked, Change::Untracked)
    );
    assert_eq!(e.path.file_name(), b"profondo.txt");
    assert!(!e.is_staged() && !e.is_unstaged());
}

#[test]
fn sottomodulo_modificato() {
    let s = parse(MISTO);
    let e = entry(&s, b"modulo");
    assert_eq!(
        e.submodule(),
        Some(SubmoduleStatus {
            commit_changed: false,
            modified: true,
            untracked: true,
        })
    );
    assert_eq!(entry(&s, b"modificato.txt").submodule(), None);
}

#[test]
fn conflitti_di_merge() {
    let s = parse(CONFLITTO);
    assert_eq!(s.upstream, None);
    assert!(s.has_conflicts());
    let conflicts: Vec<(String, Conflict)> = s
        .entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Unmerged(u) => Some((e.path.to_string(), u.conflict)),
            _ => None,
        })
        .collect();
    assert_eq!(
        conflicts,
        [
            ("cancellato-da-loro.txt".into(), Conflict::DeletedByThem),
            ("cancellato-da-noi.txt".into(), Conflict::DeletedByUs),
            ("entrambi-aggiunti.txt".into(), Conflict::BothAdded),
            ("entrambi-modificati.txt".into(), Conflict::BothModified),
        ]
    );
    let e = entry(&s, b"entrambi-aggiunti.txt");
    assert!(e.is_conflict() && !e.is_staged() && !e.is_unstaged());
    assert_eq!((e.index, e.worktree), (Change::Added, Change::Added));
    let EntryKind::Unmerged(u) = &e.kind else {
        panic!("conflitto atteso");
    };
    // Aggiunto da entrambi: niente base.
    assert_eq!((u.mode_base, u.oid_base), (0, zero()));
    assert_eq!(u.oid_ours, oid("ba2906d0666cf726c7eaadd2cd3db615dedfdf3a"));
    assert_eq!(
        u.oid_theirs,
        oid("891442819aa84f8bfce3157d86a514a2ec541b21")
    );
    assert!(entry(&s, b"pulito.txt").is_untracked());
}

#[test]
fn head_staccata() {
    let s = parse(STACCATA);
    assert_eq!(
        s.head,
        Head::Detached(oid("ae92c4f12462ba32e13dea5e0b5cd18f26f31589"))
    );
    assert_eq!(s.upstream, None);
    assert_eq!(s.entries.len(), 1);
    assert!(entry(&s, b"file.txt").is_unstaged());
}

#[test]
fn repo_vuoto() {
    let s = parse(VUOTO);
    assert_eq!(
        s,
        Status {
            head: Head::Branch {
                name: "main".into(),
                target: None,
            },
            upstream: None,
            entries: Vec::new(),
            operation: None,
        }
    );
    assert!(s.is_clean());

    let s = parse(VUOTO_FILE);
    assert_eq!(s.head.target(), None);
    assert!(entry(&s, b"nuovo.txt").is_staged());
    assert!(entry(&s, b"libero.txt").is_untracked());
}

#[test]
fn upstream_sparito_senza_ahead_behind() {
    let s = parse(UPSTREAM_SPARITO);
    assert_eq!(
        s.upstream,
        Some(Upstream {
            name: "origin/main".into(),
            ahead: None,
            behind: None,
        })
    );
    assert!(s.is_clean());
}

#[test]
fn ignorati() {
    let s = parse(IGNORATI);
    let kinds: Vec<(String, EntryKind)> = s
        .entries
        .iter()
        .map(|e| (e.path.to_string(), e.kind.clone()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("libero.txt".into(), EntryKind::Untracked),
            ("build/out.o".into(), EntryKind::Ignored),
            ("debug.log".into(), EntryKind::Ignored),
        ]
    );
    assert_eq!(xy(&s.entries[2]), "!!");
}

#[test]
fn copie() {
    let s = parse(COPIE);
    let e = entry(&s, b"copia.txt");
    assert_eq!(e.index, Change::Copied);
    let EntryKind::Copied {
        original, score, ..
    } = &e.kind
    else {
        panic!("copia attesa");
    };
    assert_eq!((original, *score), (&RepoPath::from("originale.txt"), 100));
    assert!(e.is_staged());
}

#[test]
fn intestazioni_sconosciute_ignorate() {
    let mut input = b"# stash 3\0# futuro\0".to_vec();
    input.extend_from_slice(STACCATA);
    assert_eq!(parse_status(&input), Ok(parse(STACCATA)));
}

#[test]
fn argomenti() {
    assert_eq!(
        STATUS_ARGS,
        ["--porcelain=v2", "-z", "--branch", "--untracked-files=all"]
    );
}

#[test]
fn json_dello_stato() {
    let s = parse(MISTO);
    let json = serde_json::to_value(&s).expect("serializzabile");
    assert_eq!(json["upstream"]["ahead"], 2);
    // I percorsi UTF-8 sono stringhe, gli altri array di byte.
    let paths: Vec<&serde_json::Value> = json["entries"]
        .as_array()
        .expect("array")
        .iter()
        .map(|e| &e["path"])
        .collect();
    assert_eq!(paths[0], "aggiunto.txt");
    let non_utf8 = paths
        .iter()
        .find(|p| p.is_array())
        .expect("almeno un percorso non UTF-8");
    let bytes: Vec<u8> = serde_json::from_value((*non_utf8).clone()).expect("byte");
    assert_eq!(bytes, b"nuovo-\xe9.txt");
    let back: Status = serde_json::from_value(json).expect("deserializzabile");
    assert_eq!(back, s);
}

// --- Input malformati -------------------------------------------------------------------

/// Le due intestazioni di [`STACCATA`] seguite da `record` e dal NUL finale.
fn with_record(record: &[u8]) -> Vec<u8> {
    let mut input = b"# branch.oid ae92c4f12462ba32e13dea5e0b5cd18f26f31589\0\
                      # branch.head main\0"
        .to_vec();
    input.extend_from_slice(record);
    input.push(0);
    input
}

fn err_of(input: &[u8]) -> String {
    reason(parse_status(input).unwrap_err())
}

#[test]
fn errore_senza_intestazioni() {
    assert!(err_of(b"").contains("manca l'intestazione `# branch.oid`"));
    let r = err_of(b"? a.txt\0");
    assert!(r.contains("`--branch`"), "{r}");
    let r = err_of(b"# branch.oid (initial)\0");
    assert!(r.contains("`# branch.head`"), "{r}");
}

#[test]
fn errore_se_manca_il_nul_finale() {
    let input = &STACCATA[..STACCATA.len() - 1];
    assert!(err_of(input).contains("non termina con NUL"));
}

#[test]
fn errore_su_intestazioni_non_valide() {
    let casi: [(&[u8], &str); 7] = [
        (
            b"# branch.oid 1234\0# branch.head main\0",
            "non è un id valido",
        ),
        (
            b"# branch.oid (initial)\0# branch.head (detached)\0",
            "staccata",
        ),
        (
            b"# branch.oid (initial)\0# branch.head\0",
            "senza nome del branch",
        ),
        (
            b"# branch.oid (initial)\0# branch.head main\0# branch.ab +1 -2\0",
            "senza `branch.upstream`",
        ),
        (
            b"# branch.oid (initial)\0# branch.head main\0# branch.upstream o/m\0# branch.ab 1 2\0",
            "`branch.ab 1 2` non valido",
        ),
        (
            b"# branch.oid (initial)\0# branch.head main\0# branch.upstream o/m\0# branch.ab +1\0",
            "non valido",
        ),
        (
            b"# branch.oid (initial)\0# branch.head a\0# branch.head b\0",
            "ripetuta",
        ),
    ];
    for (input, attesa) in casi {
        let r = err_of(input);
        assert!(
            r.contains(attesa),
            "{:?}: {r}",
            String::from_utf8_lossy(input)
        );
    }
}

#[test]
fn errore_su_voci_non_valide() {
    let h = "ae92c4f12462ba32e13dea5e0b5cd18f26f31589";
    let ord = |xy: &str, sub: &str, mode: &str, id: &str, path: &str| {
        format!("1 {xy} {sub} 100644 {mode} 100644 {h} {id} {path}")
    };
    let casi: Vec<(Vec<u8>, &str)> = vec![
        (ord("XY", "N...", "100644", h, "a").into(), "codice XY"),
        (ord("M", "N...", "100644", h, "a").into(), "codice XY"),
        (ord(".M", "S?..", "100644", h, "a").into(), "sottomodulo"),
        (ord(".M", "N...", "100648", h, "a").into(), "modo ottale"),
        (
            ord(".M", "N...", "100644", "zz", "a").into(),
            "id nell'indice",
        ),
        (
            format!("1 .M N... 100644 100644 100644 {h} {h}").into(),
            "7 campi invece di 8",
        ),
        (
            format!("1 .M N... 100644 100644 100644 {h} {h} ").into(),
            "percorso vuoto",
        ),
        (b"? ".to_vec(), "percorso vuoto"),
        (b"x a.txt".to_vec(), "tipo di record `x` sconosciuto"),
        (b"?a.txt".to_vec(), "non è un record valido"),
        (Vec::new(), "record vuoto"),
        (
            format!("u MM N... 100644 100644 100644 100644 {h} {h} {h} a").into(),
            "non è uno stato di conflitto",
        ),
        (
            format!("2 R. N... 100644 100644 100644 {h} {h} X100 nuovo\0vecchio").into(),
            "punteggio di rinomina",
        ),
        (
            format!("2 R. N... 100644 100644 100644 {h} {h} R101 nuovo\0vecchio").into(),
            "da 0 a 100",
        ),
        (
            format!("2 R. N... 100644 100644 100644 {h} {h} R90 nuovo\0").into(),
            "percorso originale: percorso vuoto",
        ),
    ];
    for (record, attesa) in casi {
        let r = err_of(&with_record(&record));
        assert!(r.contains("record n. 3"), "{r}");
        assert!(
            r.contains(attesa),
            "{:?}: {r}",
            String::from_utf8_lossy(&record)
        );
    }
    // Il percorso originale della rinomina manca del tutto: l'output finisce dopo il nuovo.
    let mut input =
        with_record(format!("2 R. N... 100644 100644 100644 {h} {h} R90 nuovo").as_bytes());
    let r = err_of(&input);
    assert!(r.contains("manca il percorso originale"), "{r}");
    input.extend_from_slice(b"vecchio\0");
    assert!(parse_status(&input).is_ok());
}

// --- Proprietà --------------------------------------------------------------------------

/// Riscrive uno stato come farebbe git con [`STATUS_ARGS`] (e `--ignored`).
fn render(s: &Status) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rec = |parts: &[&[u8]]| {
        for p in parts {
            out.extend_from_slice(p);
        }
        out.push(0);
    };
    let oid_text = match s.head.target() {
        Some(o) => o.to_string(),
        None => "(initial)".to_owned(),
    };
    rec(&[b"# branch.oid ", oid_text.as_bytes()]);
    let name = match &s.head {
        Head::Branch { name, .. } => name.clone(),
        Head::Detached(_) => "(detached)".to_owned(),
    };
    rec(&[b"# branch.head ", name.as_bytes()]);
    if let Some(u) = &s.upstream {
        rec(&[b"# branch.upstream ", u.name.as_bytes()]);
        if let (Some(a), Some(b)) = (u.ahead, u.behind) {
            rec(&[format!("# branch.ab +{a} -{b}").as_bytes()]);
        }
    }
    let sub = |s: Option<SubmoduleStatus>| match s {
        None => "N...".to_owned(),
        Some(s) => format!(
            "S{}{}{}",
            if s.commit_changed { 'C' } else { '.' },
            if s.modified { 'M' } else { '.' },
            if s.untracked { 'U' } else { '.' }
        ),
    };
    let tracked = |i: &TrackedInfo| {
        format!(
            "{} {:06o} {:06o} {:06o} {} {}",
            sub(i.submodule),
            i.mode_head,
            i.mode_index,
            i.mode_worktree,
            i.oid_head,
            i.oid_index
        )
    };
    for e in &s.entries {
        let xy = format!("{}{}", e.index.letter(), e.worktree.letter());
        let path = e.path.as_bytes();
        match &e.kind {
            EntryKind::Ordinary(i) => {
                rec(&[format!("1 {xy} {} ", tracked(i)).as_bytes(), path]);
            }
            EntryKind::Renamed {
                info,
                original,
                score,
            }
            | EntryKind::Copied {
                info,
                original,
                score,
            } => {
                let letter = if matches!(e.kind, EntryKind::Copied { .. }) {
                    'C'
                } else {
                    'R'
                };
                rec(&[
                    format!("2 {xy} {} {letter}{score} ", tracked(info)).as_bytes(),
                    path,
                ]);
                rec(&[original.as_bytes()]);
            }
            EntryKind::Unmerged(u) => rec(&[
                format!(
                    "u {xy} {} {:06o} {:06o} {:06o} {:06o} {} {} {} ",
                    sub(u.submodule),
                    u.mode_base,
                    u.mode_ours,
                    u.mode_theirs,
                    u.mode_worktree,
                    u.oid_base,
                    u.oid_ours,
                    u.oid_theirs
                )
                .as_bytes(),
                path,
            ]),
            EntryKind::Untracked => rec(&[b"? ", path]),
            EntryKind::Ignored => rec(&[b"! ", path]),
        }
    }
    out
}

fn arb_oid() -> impl Strategy<Value = Oid> {
    any::<[u8; 20]>().prop_map(Oid::from_bytes)
}

/// Byte qualsiasi tranne NUL, almeno uno: un percorso valido per git con `-z`.
fn arb_path() -> impl Strategy<Value = RepoPath> {
    prop::collection::vec(1u8..=255, 1..20).prop_map(RepoPath::from)
}

fn arb_change() -> impl Strategy<Value = Change> {
    prop::sample::select(vec![
        Change::Unmodified,
        Change::Modified,
        Change::TypeChanged,
        Change::Added,
        Change::Deleted,
        Change::Renamed,
        Change::Copied,
    ])
}

fn arb_sub() -> impl Strategy<Value = Option<SubmoduleStatus>> {
    prop::option::of(
        any::<(bool, bool, bool)>().prop_map(|(c, m, u)| SubmoduleStatus {
            commit_changed: c,
            modified: m,
            untracked: u,
        }),
    )
}

fn arb_mode() -> impl Strategy<Value = u32> {
    prop::sample::select(vec![0, 0o100644, 0o100755, 0o120000, 0o160000])
}

fn arb_tracked() -> impl Strategy<Value = TrackedInfo> {
    (
        arb_sub(),
        arb_mode(),
        arb_mode(),
        arb_mode(),
        arb_oid(),
        arb_oid(),
    )
        .prop_map(|(submodule, mh, mi, mw, oh, oi)| TrackedInfo {
            submodule,
            mode_head: mh,
            mode_index: mi,
            mode_worktree: mw,
            oid_head: oh,
            oid_index: oi,
        })
}

fn arb_conflict() -> impl Strategy<Value = (Conflict, Change, Change)> {
    use Change::{Added as A, Deleted as D, Unmerged as U};
    prop::sample::select(vec![
        (Conflict::BothDeleted, D, D),
        (Conflict::AddedByUs, A, U),
        (Conflict::DeletedByThem, U, D),
        (Conflict::AddedByThem, U, A),
        (Conflict::DeletedByUs, D, U),
        (Conflict::BothAdded, A, A),
        (Conflict::BothModified, U, U),
    ])
}

fn arb_entry() -> impl Strategy<Value = StatusEntry> {
    let tracked = (arb_path(), arb_change(), arb_change(), arb_tracked()).prop_map(
        |(path, index, worktree, info)| StatusEntry {
            path,
            index,
            worktree,
            kind: EntryKind::Ordinary(info),
        },
    );
    let moved = (
        arb_path(),
        arb_change(),
        arb_tracked(),
        arb_path(),
        0u8..=100,
        any::<bool>(),
    )
        .prop_map(|(path, worktree, info, original, score, copy)| {
            let (index, kind) = if copy {
                (
                    Change::Copied,
                    EntryKind::Copied {
                        info,
                        original,
                        score,
                    },
                )
            } else {
                (
                    Change::Renamed,
                    EntryKind::Renamed {
                        info,
                        original,
                        score,
                    },
                )
            };
            StatusEntry {
                path,
                index,
                worktree,
                kind,
            }
        });
    let unmerged = (
        arb_path(),
        arb_conflict(),
        arb_sub(),
        (arb_mode(), arb_mode(), arb_mode(), arb_mode()),
        (arb_oid(), arb_oid(), arb_oid()),
    )
        .prop_map(
            |(path, (conflict, index, worktree), submodule, (m1, m2, m3, mw), (h1, h2, h3))| {
                StatusEntry {
                    path,
                    index,
                    worktree,
                    kind: EntryKind::Unmerged(UnmergedInfo {
                        conflict,
                        submodule,
                        mode_base: m1,
                        mode_ours: m2,
                        mode_theirs: m3,
                        mode_worktree: mw,
                        oid_base: h1,
                        oid_ours: h2,
                        oid_theirs: h3,
                    }),
                }
            },
        );
    let simple = (arb_path(), any::<bool>()).prop_map(|(path, ignored)| {
        let (change, kind) = if ignored {
            (Change::Ignored, EntryKind::Ignored)
        } else {
            (Change::Untracked, EntryKind::Untracked)
        };
        StatusEntry {
            path,
            index: change,
            worktree: change,
            kind,
        }
    });
    prop_oneof![tracked, moved, unmerged, simple]
}

fn arb_head() -> impl Strategy<Value = Head> {
    prop_oneof![
        ("[a-z][a-z/._-]{0,15}", prop::option::of(arb_oid()))
            .prop_map(|(name, target)| Head::Branch { name, target }),
        arb_oid().prop_map(Head::Detached),
    ]
}

fn arb_upstream() -> impl Strategy<Value = Option<Upstream>> {
    prop::option::of(
        (
            "[a-z][a-z/]{0,15}",
            prop::option::of((any::<u32>(), any::<u32>())),
        )
            .prop_map(|(name, ab)| Upstream {
                name,
                ahead: ab.map(|(a, _)| a),
                behind: ab.map(|(_, b)| b),
            }),
    )
}

fn arb_status() -> impl Strategy<Value = Status> {
    (
        arb_head(),
        arb_upstream(),
        prop::collection::vec(arb_entry(), 0..6),
    )
        .prop_map(|(head, upstream, entries)| Status {
            head,
            upstream,
            entries,
            operation: None,
        })
}

proptest! {
    #[test]
    fn round_trip_del_formato(status in arb_status()) {
        let parsed = parse_status(&render(&status))
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(parsed, status);
    }

    #[test]
    fn round_trip_json(status in arb_status()) {
        let json = serde_json::to_string(&status).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let back: Status = serde_json::from_str(&json).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(back, status);
    }

    #[test]
    fn nessun_panico_su_byte_qualsiasi(input in prop::collection::vec(any::<u8>(), 0..400)) {
        let _ = parse_status(&input);
    }

    #[test]
    fn nessun_panico_su_output_reale_alterato(pos in 0..MISTO.len(), byte in any::<u8>()) {
        let mut input = MISTO.to_vec();
        input[pos] = byte;
        let _ = parse_status(&input);
    }

    #[test]
    fn nessun_panico_su_output_reale_troncato(len in 0..MISTO.len()) {
        let _ = parse_status(&MISTO[..len]);
    }
}
