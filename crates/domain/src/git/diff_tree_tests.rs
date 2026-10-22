use super::*;
use crate::DomainError;
use proptest::prelude::*;

// Output reale di git 2.43.0, generato da `testdata/diff-tree.sh` (`sh diff-tree.sh testdata`)
// in un repo temporaneo con `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1` e date fisse.
// Il comando è sempre `git diff-tree` con gli argomenti di [`DIFF_TREE_ARGS`] e le revisioni.

/// Commit radice (`--root`): dieci file aggiunti, fra cui `a\ncapo.txt`, `caff\xff.txt` (non
/// UTF-8), `con spazio.txt` e `src/annidata/profondo.rs`.
const RADICE: &[u8] = include_bytes!("testdata/diff-tree-radice.bin");
/// Rispetto al genitore: eliminato, modificato, rinomina esatta, reso eseguibile, rinomina con
/// modifiche, aggiunto in una sottocartella, file diventato link simbolico.
const MISTO: &[u8] = include_bytes!("testdata/diff-tree-misto.bin");
/// Copia di un file, con `-C --find-copies-harder` in più.
const COPIE: &[u8] = include_bytes!("testdata/diff-tree-copie.bin");
/// Commit vuoto (`--allow-empty`): zero byte.
const VUOTO: &[u8] = include_bytes!("testdata/diff-tree-vuoto.bin");
/// Merge confrontato con il primo genitore: solo il file portato dal branch unito.
const MERGE: &[u8] = include_bytes!("testdata/diff-tree-merge.bin");
/// Lo script che genera i file: deve usare gli stessi argomenti di [`DIFF_TREE_ARGS`].
const SCRIPT: &str = include_str!("testdata/diff-tree.sh");

const FILE: u32 = 0o100644;
const ZERO: Oid = Oid::from_bytes([0; Oid::LEN]);

fn oid(hex: &str) -> Oid {
    Oid::from_hex(hex.as_bytes()).expect("oid di test valido")
}

fn parse(input: &[u8]) -> Vec<ChangedFile> {
    parse_diff_tree(input).expect("output reale valido")
}

fn reason(err: DomainError) -> String {
    match err {
        DomainError::InvalidGitOutput { command, reason } => {
            assert_eq!(command, "diff-tree");
            reason
        }
        other => panic!("errore inatteso: {other:?}"),
    }
}

fn paths(files: &[ChangedFile]) -> Vec<&[u8]> {
    files.iter().map(|f| f.path.as_bytes()).collect()
}

#[test]
fn lo_script_usa_gli_argomenti_del_parser() {
    assert!(SCRIPT.contains(&format!(
        "git diff-tree {} \"$@\"",
        DIFF_TREE_ARGS.join(" ")
    )));
}

#[test]
fn commit_radice_tutti_aggiunti() {
    let files = parse(RADICE);
    assert_eq!(
        paths(&files),
        [
            &b"a\ncapo.txt"[..],
            b"caff\xff.txt",
            b"con spazio.txt",
            b"eliminato.txt",
            b"modificato.txt",
            b"script.sh",
            b"sposta.txt",
            b"src/annidata/profondo.rs",
            b"tipo.txt",
            b"vecchio.txt",
        ]
    );
    for f in &files {
        assert_eq!(f.change, Change::Added, "{}", f.path);
        assert_eq!((f.old_mode, f.new_mode), (0, FILE));
        assert_eq!(f.old_oid, ZERO);
        assert_ne!(f.new_oid, ZERO);
        assert_eq!((&f.original, f.score), (&None, None));
        assert_eq!(f.old_path(), &f.path);
    }
    // Il byte non UTF-8 resta tale; solo la visualizzazione lo sostituisce.
    assert_eq!(files[1].path.to_str(), None);
    assert_eq!(files[1].path.to_string_lossy(), "caff\u{fffd}.txt");
    assert_eq!(
        files[4].new_oid,
        oid("913caa9051271045c0828025d3ce0d40ebbc9f95")
    );
}

#[test]
fn ogni_tipo_di_modifica() {
    let files = parse(MISTO);
    let summary: Vec<_> = files
        .iter()
        .map(|f| {
            (
                f.change,
                f.original.as_ref().and_then(RepoPath::to_str),
                f.path.to_str().unwrap(),
                f.score,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (Change::Deleted, None, "eliminato.txt", None),
            (Change::Modified, None, "modificato.txt", None),
            (Change::Renamed, Some("vecchio.txt"), "nuovo.txt", Some(100)),
            (Change::Modified, None, "script.sh", None),
            (
                Change::Renamed,
                Some("sposta.txt"),
                "spostato.txt",
                Some(86)
            ),
            (Change::Added, None, "src/aggiunto.rs", None),
            (Change::TypeChanged, None, "tipo.txt", None),
        ]
    );

    let deleted = &files[0];
    assert_eq!((deleted.old_mode, deleted.new_mode), (FILE, 0));
    assert_eq!(
        (deleted.old_oid, deleted.new_oid),
        (oid("f920a38e404dfceb98f8f4ed5caa06811e67d668"), ZERO)
    );

    let modified = &files[1];
    assert_eq!(
        (modified.old_oid, modified.new_oid),
        (
            oid("913caa9051271045c0828025d3ce0d40ebbc9f95"),
            oid("5c296212396dfd8d045eb394ad827528451c02d2")
        )
    );

    let exact = &files[2];
    assert_eq!(exact.old_oid, exact.new_oid);
    assert_eq!(exact.old_path().to_str(), Some("vecchio.txt"));

    // Solo il modo: stesso contenuto, reso eseguibile.
    let script = &files[3];
    assert_eq!((script.old_mode, script.new_mode), (FILE, 0o100755));
    assert_eq!(script.old_oid, script.new_oid);

    let renamed = &files[4];
    assert_ne!(renamed.old_oid, renamed.new_oid);

    let link = &files[6];
    assert_eq!((link.old_mode, link.new_mode), (FILE, 0o120000));
    assert!(files.iter().all(|f| !f.is_submodule()));
}

#[test]
fn copia_con_due_percorsi() {
    let files = parse(COPIE);
    assert_eq!(files.len(), 1);
    let copy = &files[0];
    assert_eq!(copy.change, Change::Copied);
    assert_eq!(copy.score, Some(100));
    assert_eq!(
        copy.original.as_ref().and_then(RepoPath::to_str),
        Some("modificato.txt")
    );
    assert_eq!(copy.path.to_str(), Some("copia.txt"));
}

#[test]
fn commit_vuoto_e_merge() {
    assert_eq!(VUOTO, b"");
    assert!(parse(VUOTO).is_empty());

    let files = parse(MERGE);
    assert_eq!(paths(&files), [b"laterale.txt"]);
    assert_eq!(files[0].change, Change::Added);
}

#[test]
fn sottomodulo() {
    let input = b":000000 160000 0000000000000000000000000000000000000000 \
                  1111111111111111111111111111111111111111 A\0libs/sub\0";
    let files = parse(input);
    assert!(files[0].is_submodule());
    assert_eq!(files[0].new_mode, 0o160000);
}

#[test]
fn json_dei_dettagli() {
    let files = parse(MISTO);
    let value = serde_json::to_value(&files[4]).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "path": "spostato.txt",
            "change": "Renamed",
            "original": "sposta.txt",
            "score": 86,
            "old_mode": FILE,
            "new_mode": FILE,
            "old_oid": "7335c645a2ec265ad9402bea6b1ef5373583d0fb",
            "new_oid": "9a51ecd21a2604ec37dd3b7dfa4441c2244a3d01",
        })
    );
    let back: ChangedFile = serde_json::from_value(value).unwrap();
    assert_eq!(back, files[4]);
    // Un percorso non UTF-8 diventa un array di byte e torna identico.
    let latin = &parse(RADICE)[1];
    let value = serde_json::to_value(latin).unwrap();
    assert!(value["path"].is_array(), "{value}");
    assert_eq!(
        serde_json::from_value::<ChangedFile>(value).unwrap(),
        *latin
    );
}

#[test]
fn errori() {
    let header = ":100644 100644 913caa9051271045c0828025d3ce0d40ebbc9f95 \
                  5c296212396dfd8d045eb394ad827528451c02d2";
    let cases: &[(Vec<u8>, &str)] = &[
        (b"senza nul".to_vec(), "non termina con NUL"),
        (format!("{header} M\0").into_bytes(), "manca il percorso"),
        (format!("{header} M\0\0").into_bytes(), "il percorso vuoto"),
        (
            format!("{header} R100\0a\0").into_bytes(),
            "manca il percorso di destinazione",
        ),
        (format!("{header} X\0a\0").into_bytes(), "stato `X`"),
        (format!("{header} M50\0a\0").into_bytes(), "`M50` inatteso"),
        (format!("{header} R\0a\0b\0").into_bytes(), "punteggio"),
        (format!("{header} R101\0a\0b\0").into_bytes(), "punteggio"),
        (format!("{header} \0a\0").into_bytes(), "stato vuoto"),
        (b"100644 M\0a\0".to_vec(), "deve iniziare con `:`"),
        (b":100644 M\0a\0".to_vec(), "2 campi invece di 5"),
        (
            b":100648 100644 913caa9051271045c0828025d3ce0d40ebbc9f95 \
              5c296212396dfd8d045eb394ad827528451c02d2 M\0a\0"
                .to_vec(),
            "modo ottale",
        ),
        (
            b":100644 100644 xyz 5c296212396dfd8d045eb394ad827528451c02d2 M\0a\0".to_vec(),
            "id vecchio",
        ),
    ];
    for (input, expected) in cases {
        let err = reason(parse_diff_tree(input).unwrap_err());
        assert!(err.contains(expected), "{expected}: {err}");
    }
    // Il numero del record è quello del record sbagliato.
    let mut input = MISTO.to_vec();
    input.extend_from_slice(b":rotto\0x\0");
    let err = reason(parse_diff_tree(&input).unwrap_err());
    assert!(err.starts_with("record n. 8:"), "{err}");
}

proptest! {
    /// Qualunque percorso senza NUL torna identico, anche con a capo, spazi e byte non UTF-8.
    #[test]
    fn percorsi_arbitrari(
        a in proptest::collection::vec(1u8..=255, 1..40),
        b in proptest::collection::vec(1u8..=255, 1..40),
    ) {
        let header = ":100644 100644 913caa9051271045c0828025d3ce0d40ebbc9f95 \
                      5c296212396dfd8d045eb394ad827528451c02d2";
        let mut input = format!("{header} R075\0").into_bytes();
        input.extend_from_slice(&a);
        input.push(0);
        input.extend_from_slice(&b);
        input.push(0);
        let files = parse_diff_tree(&input).unwrap();
        prop_assert_eq!(files.len(), 1);
        prop_assert_eq!(files[0].old_path().as_bytes(), &a[..]);
        prop_assert_eq!(files[0].path.as_bytes(), &b[..]);
    }

    /// Nessun input fa andare in panico il parser.
    #[test]
    fn mai_panico(input in proptest::collection::vec(any::<u8>(), 0..200)) {
        let _ = parse_diff_tree(&input);
    }
}
