use super::*;
use crate::DomainError;
use proptest::prelude::*;

// Output reale di git 2.43.0, generato da `testdata/diff.sh` (`sh diff.sh testdata`) in un repo
// temporaneo con `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1` e date fisse. Il comando
// è `git -c core.quotePath=false diff` con gli argomenti di [`DIFF_ARGS`], più `--cached` o
// niente (worktree); le eccezioni sono indicate su ogni costante.

/// Worktree: più hunk con intestazione di contesto (`codice.c`), conteggi omessi
/// (`@@ -1 +1 @@`), a capo finale aggiunto, tolto e assente nei due lati, CRLF, contenuto non
/// UTF-8, righe che sembrano intestazioni (`--- a/finta`, `diff --git`), riga vuota di contesto.
const TESTO: &[u8] = include_bytes!("testdata/diff-testo.bin");
/// Le stesse modifiche di [`TESTO`] con una configurazione ostile (`diff.noprefix`,
/// `diff.mnemonicPrefix`, `diff.external`, `diff.renames=false`, `diff.relative`,
/// `diff.submodule=log`, `color.ui=always`, `core.quotePath=true` nel file di configurazione e
/// `diff.suppressBlankEmpty`): differisce solo per la riga di contesto vuota senza spazio.
const CONFIG: &[u8] = include_bytes!("testdata/diff-config.bin");
/// `--cached`: binari nuovi, eliminati e modificati, file eliminato, cambio di solo modo, file
/// nuovo, rinomina esatta, rinomina con modifiche, rinomina con cambio di modo, file diventato
/// link simbolico (eliminato + nuovo), file vuoto rinominato (eliminato vuoto + nuovo vuoto).
const FILE: &[u8] = include_bytes!("testdata/diff-file.bin");
/// `--cached -C --find-copies-harder`: una copia esatta.
const COPIE: &[u8] = include_bytes!("testdata/diff-copie.bin");
/// `--cached`: nomi con a capo, byte non UTF-8 (`caff\xff.txt`), tab, virgolette, spazio,
/// backslash, UTF-8 (`perché.txt`) e una rinomina da un nome semplice a uno tra virgolette.
const NOMI: &[u8] = include_bytes!("testdata/diff-nomi.bin");
/// Come [`NOMI`] ma con `git diff` senza `-c core.quotePath=false`: byte non ASCII in ottale.
const NOMI_QUOTEPATH: &[u8] = include_bytes!("testdata/diff-nomi-quotepath.bin");
/// `--cached`: sottomodulo aggiunto (con `.gitmodules`).
const SOTTOMODULO_NUOVO: &[u8] = include_bytes!("testdata/diff-sottomodulo-nuovo.bin");
/// Worktree: sottomodulo spostato su un altro commit, con `-c diff.submodule=log` in più.
const SOTTOMODULO: &[u8] = include_bytes!("testdata/diff-sottomodulo.bin");
/// Worktree senza modifiche: zero byte.
const VUOTO: &[u8] = include_bytes!("testdata/diff-vuoto.bin");
/// Worktree durante un merge con due conflitti (uno con un tab nel nome) e un file modificato.
const CONFLITTO: &[u8] = include_bytes!("testdata/diff-conflitto.bin");
/// `--cached` durante lo stesso merge: solo righe `* Unmerged path`.
const CONFLITTO_INDICE: &[u8] = include_bytes!("testdata/diff-conflitto-indice.bin");
/// Lo script che genera i file: deve usare gli stessi argomenti di [`DIFF_ARGS`].
const SCRIPT: &str = include_str!("testdata/diff.sh");

const EMPTY_BLOB: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
const ZERO: Oid = Oid::from_bytes([0; Oid::LEN]);

fn parse(input: &[u8]) -> Diff {
    parse_diff(input).expect("output reale valido")
}

fn reason(input: &[u8]) -> String {
    match parse_diff(input) {
        Err(DomainError::InvalidGitOutput { command, reason }) => {
            assert_eq!(command, "diff");
            reason
        }
        other => panic!("errore atteso, ottenuto {other:?}"),
    }
}

fn file<'a>(diff: &'a Diff, path: &[u8]) -> &'a FileDiff {
    let mut found = diff.files.iter().filter(|f| f.new_path.as_bytes() == path);
    let f = found.next().expect("file presente");
    assert!(found.next().is_none(), "un solo file con questo percorso");
    f
}

/// Le righe di un hunk come `(prefisso, contenuto, vecchia, nuova, senza a capo)`.
type Row<'a> = (char, &'a [u8], Option<u32>, Option<u32>, bool);

fn rows(hunk: &Hunk) -> Vec<Row<'_>> {
    hunk.lines
        .iter()
        .map(|l| {
            (
                char::from(l.kind.prefix()),
                l.content.as_bytes(),
                l.old_line,
                l.new_line,
                l.no_newline,
            )
        })
        .collect()
}

fn ranges(hunk: &Hunk) -> (u32, u32, u32, u32) {
    (
        hunk.old_start,
        hunk.old_count,
        hunk.new_start,
        hunk.new_count,
    )
}

#[test]
fn lo_script_usa_gli_argomenti_del_parser() {
    let args = DIFF_ARGS.join(" ");
    assert!(SCRIPT.contains(&format!("git -c core.quotePath=false diff {args} \"$@\"")));
    assert!(SCRIPT.contains(&format!("git diff {args} --cached")));
    assert!(SCRIPT.contains(&format!("git -c diff.submodule=log diff {args} >")));
}

#[test]
fn output_vuoto() {
    assert_eq!(parse(VUOTO), Diff::default());
    assert!(VUOTO.is_empty());
}

#[test]
fn testo_hunk_e_numeri_di_riga() {
    let diff = parse(TESTO);
    let paths: Vec<&[u8]> = diff.files.iter().map(|f| f.path().as_bytes()).collect();
    assert_eq!(
        paths,
        [
            &b"aggiunge-a-capo.txt"[..],
            b"codice.c",
            b"crlf.txt",
            b"entrambi-senza.txt",
            b"finte-intestazioni.txt",
            b"latin1.txt",
            b"riga-vuota.txt",
            b"toglie-a-capo.txt",
            b"una-riga.txt",
        ]
    );
    for f in &diff.files {
        assert_eq!(f.change, Change::Modified);
        assert_eq!(f.old_path, f.new_path);
        assert_eq!((f.old_mode, f.new_mode), (Some(0o100644), Some(0o100644)));
        assert!(f.old_oid.is_some() && f.new_oid.is_some() && f.old_oid != f.new_oid);
        assert!(!f.binary && !f.is_submodule() && f.similarity.is_none());
    }

    let codice = file(&diff, b"codice.c");
    assert_eq!(
        codice.old_oid,
        Some(Oid::from_hex(b"1b61b629b9fa56ee14773760cf2a615eadd7c5c7").unwrap())
    );
    let [primo, secondo] = &codice.hunks[..] else {
        panic!("due hunk attesi")
    };
    assert_eq!(ranges(primo), (1, 6, 1, 6));
    assert!(primo.section.is_empty());
    assert_eq!(
        rows(primo)[2],
        ('-', &b"    riga(2);"[..], Some(3), None, false)
    );
    assert_eq!(
        rows(primo)[3],
        ('+', &b"    riga(2); // cambiata"[..], None, Some(3), false)
    );
    assert_eq!(ranges(secondo), (17, 6, 17, 7));
    assert_eq!(secondo.section.as_bytes(), b"int main() {");
    assert_eq!(
        rows(secondo)[2..5],
        [
            (' ', &b"    riga(18);"[..], Some(19), Some(19), false),
            ('+', b"    nuova();", None, Some(20), false),
            (' ', b"    riga(19);", Some(20), Some(21), false),
        ]
    );
    assert_eq!(codice.line_counts(), (2, 1));

    // `@@ -1 +1 @@`: conteggi omessi, cioè 1.
    let una = &file(&diff, b"una-riga.txt").hunks[0];
    assert_eq!(ranges(una), (1, 1, 1, 1));
    assert_eq!(
        rows(una),
        [
            ('-', &b"solo"[..], Some(1), None, false),
            ('+', b"sola", None, Some(1), false),
        ]
    );
}

#[test]
fn testo_fine_file_senza_a_capo() {
    let diff = parse(TESTO);
    let aggiunge = &file(&diff, b"aggiunge-a-capo.txt").hunks[0];
    assert_eq!(
        rows(aggiunge),
        [
            (' ', &b"resta"[..], Some(1), Some(1), false),
            ('-', b"senza a capo", Some(2), None, true),
            ('+', b"senza a capo", None, Some(2), false),
        ]
    );
    let toglie = &file(&diff, b"toglie-a-capo.txt").hunks[0];
    assert_eq!(
        rows(toglie)[1..],
        [
            ('-', &b"con a capo"[..], Some(2), None, false),
            ('+', b"con a capo", None, Some(2), true),
        ]
    );
    let entrambi = &file(&diff, b"entrambi-senza.txt").hunks[0];
    assert_eq!(
        rows(entrambi),
        [
            ('-', &b"vecchia senza a capo"[..], Some(1), None, true),
            ('+', b"nuova senza a capo", None, Some(1), true),
        ]
    );
}

#[test]
fn testo_contenuto_in_byte() {
    let diff = parse(TESTO);
    let crlf = &file(&diff, b"crlf.txt").hunks[0];
    assert_eq!(rows(crlf)[2].1, b"DUE\r");
    let latin1 = &file(&diff, b"latin1.txt").hunks[0];
    assert_eq!(rows(latin1)[0].1, b"caff\xe8");
    assert_eq!(latin1.lines[0].content.to_str(), None);
    assert_eq!(latin1.lines[0].content.to_string_lossy(), "caff\u{fffd}");

    // Le righe che sembrano intestazioni restano righe dell'hunk: le conta l'intestazione.
    let finte = &file(&diff, b"finte-intestazioni.txt").hunks[0];
    assert_eq!(
        rows(finte),
        [
            (' ', &b"testa"[..], Some(1), Some(1), false),
            ('-', b"-- trattini", Some(2), None, false),
            ('-', "++ più".as_bytes(), Some(3), None, false),
            ('+', b"--- a/finta", None, Some(2), false),
            ('+', b"+++ b/finta", None, Some(3), false),
            ('+', b"diff --git a/x b/x", None, Some(4), false),
            (' ', b"coda", Some(4), Some(5), false),
        ]
    );
    let vuota = &file(&diff, b"riga-vuota.txt").hunks[0];
    assert_eq!(rows(vuota)[1], (' ', &b""[..], Some(2), Some(2), false));
}

#[test]
fn configurazione_ostile_neutralizzata() {
    assert_ne!(CONFIG, TESTO);
    assert_eq!(parse(CONFIG), parse(TESTO));
}

#[test]
fn file_nuovi_eliminati_rinominati_binari() {
    let diff = parse(FILE);
    let summary: Vec<(&[u8], &[u8], Change)> = diff
        .files
        .iter()
        .map(|f| (f.old_path.as_bytes(), f.new_path.as_bytes(), f.change))
        .collect();
    assert_eq!(
        summary,
        [
            (
                &b"binario-eliminato.bin"[..],
                &b"binario-eliminato.bin"[..],
                Change::Deleted
            ),
            (b"binario-nuovo.bin", b"binario-nuovo.bin", Change::Added),
            (b"eliminato.txt", b"eliminato.txt", Change::Deleted),
            (b"eseguibile.sh", b"eseguibile.sh", Change::Modified),
            (b"immagine.gif", b"immagine.gif", Change::Modified),
            (b"nuovo.txt", b"nuovo.txt", Change::Added),
            (
                b"rinomina-esatta.txt",
                b"rinominata-esatta.txt",
                Change::Renamed
            ),
            (
                b"rinomina-modificata.txt",
                b"rinominata-modificata.txt",
                Change::Renamed
            ),
            (b"rinomina-modo.sh", b"rinominato-modo.sh", Change::Renamed),
            (b"tipo.txt", b"tipo.txt", Change::Deleted),
            (b"tipo.txt", b"tipo.txt", Change::Added),
            (b"vuoto-eliminato.txt", b"vuoto-nuovo.txt", Change::Renamed),
        ]
    );
    let f = &diff.files;

    // Binari: niente hunk.
    assert!(f[0].binary && f[0].hunks.is_empty());
    assert_eq!((f[0].old_mode, f[0].new_mode), (Some(0o100644), None));
    assert_eq!(f[0].new_oid, Some(ZERO));
    assert!(f[1].binary && f[1].hunks.is_empty());
    assert_eq!((f[1].old_mode, f[1].new_mode), (None, Some(0o100644)));
    assert_eq!(f[1].old_oid, Some(ZERO));
    assert!(f[4].binary && f[4].hunks.is_empty());
    assert_eq!(
        (f[4].old_mode, f[4].new_mode),
        (Some(0o100644), Some(0o100644))
    );

    // Eliminato: un hunk che toglie tutto.
    assert_eq!(ranges(&f[2].hunks[0]), (1, 2, 0, 0));
    assert_eq!(f[2].line_counts(), (0, 2));

    // Cambio di solo modo: niente `index` né hunk.
    assert_eq!(
        (f[3].old_mode, f[3].new_mode),
        (Some(0o100644), Some(0o100755))
    );
    assert_eq!((f[3].old_oid, f[3].new_oid), (None, None));
    assert!(f[3].hunks.is_empty() && !f[3].binary);

    // Nuovo: `@@ -0,0 +1 @@`.
    assert_eq!(ranges(&f[5].hunks[0]), (0, 0, 1, 1));
    assert_eq!(
        rows(&f[5].hunks[0]),
        [('+', &b"nuovo"[..], None, Some(1), false)]
    );

    // Rinomina esatta: niente modi né id.
    assert_eq!(f[6].similarity, Some(100));
    assert_eq!((f[6].old_mode, f[6].old_oid), (None, None));
    assert!(f[6].hunks.is_empty());

    // Rinomina con modifiche: hunk con contesto.
    assert_eq!(f[7].similarity, Some(86));
    assert_eq!(ranges(&f[7].hunks[0]), (2, 3, 2, 4));
    assert_eq!(
        f[7].hunks[0].section.as_bytes(),
        b"contenuto modificato poco, abbastanza lungo"
    );

    // Rinomina con cambio di modo.
    assert_eq!(
        (f[8].old_mode, f[8].new_mode),
        (Some(0o100644), Some(0o100755))
    );
    assert_eq!(f[8].similarity, Some(100));

    // Da file a link simbolico: eliminato + nuovo con modo 120000 e destinazione senza a capo.
    assert_eq!(f[10].new_mode, Some(0o120000));
    assert_eq!(
        rows(&f[10].hunks[0]),
        [('+', &b"nuovo.txt"[..], None, Some(1), true)]
    );

    // File vuoto rinominato: rinomina esatta, nessun hunk.
    assert_eq!(f[11].similarity, Some(100));
    assert!(f[11].hunks.is_empty());
}

#[test]
fn copie() {
    let diff = parse(COPIE);
    let [copia] = &diff.files[..] else {
        panic!("un file atteso")
    };
    assert_eq!(copia.change, Change::Copied);
    assert_eq!(copia.old_path.as_bytes(), b"nuovo.txt");
    assert_eq!(copia.new_path.as_bytes(), b"copia.txt");
    assert_eq!(copia.similarity, Some(100));
    assert!(copia.hunks.is_empty());
}

#[test]
fn nomi_con_virgolette_spazi_e_byte_non_utf8() {
    let diff = parse(NOMI);
    let paths: Vec<&[u8]> = diff.files.iter().map(|f| f.path().as_bytes()).collect();
    assert_eq!(
        paths,
        [
            &b"a\ncapo.txt"[..],
            b"caff\xff.txt",
            b"con\ttab.txt",
            b"con \"virgolette\".txt",
            b"con spazio.txt",
            b"con\\back.txt",
            "perché.txt".as_bytes(),
            b"strano \"nome\"\tcon spazio.txt",
        ]
    );
    for f in &diff.files[..7] {
        assert_eq!(f.change, Change::Modified);
        assert_eq!(f.old_path, f.new_path);
        assert_eq!(f.line_counts(), (1, 0), "{}", f.path());
    }
    let rinomina = &diff.files[7];
    assert_eq!(rinomina.change, Change::Renamed);
    assert_eq!(rinomina.old_path.as_bytes(), b"semplice.txt");
}

#[test]
fn nomi_con_quote_path_ottale() {
    // Stessi nomi decodificati dagli escape ottali; riscritti con `core.quotePath=false`.
    let diff = parse(NOMI_QUOTEPATH);
    assert_eq!(diff, parse(NOMI));
    assert_eq!(diff.to_patch(), NOMI);
}

#[test]
fn sottomoduli() {
    let nuovo = parse(SOTTOMODULO_NUOVO);
    let gitmodules = file(&nuovo, b".gitmodules");
    assert_eq!(gitmodules.change, Change::Added);
    assert!(!gitmodules.is_submodule());
    let sub = file(&nuovo, b"sub");
    assert_eq!(sub.change, Change::Added);
    assert!(sub.is_submodule());
    assert_eq!(
        rows(&sub.hunks[0]),
        [(
            '+',
            &b"Subproject commit ea5d87686ebd982090a0144746c90588190de864"[..],
            None,
            Some(1),
            false
        )]
    );

    let aggiornato = parse(SOTTOMODULO);
    let [sub] = &aggiornato.files[..] else {
        panic!("un file atteso")
    };
    assert_eq!(sub.change, Change::Modified);
    assert!(sub.is_submodule());
    assert_eq!(
        (sub.old_mode, sub.new_mode),
        (Some(0o160000), Some(0o160000))
    );
    assert_eq!(
        sub.new_oid,
        Some(Oid::from_hex(b"a7224e88790f507a99ee8b079c46759be15e9485").unwrap())
    );
    assert_eq!(sub.line_counts(), (1, 1));
}

#[test]
fn conflitti_senza_hunk() {
    let diff = parse(CONFLITTO);
    let summary: Vec<(&[u8], Change, usize)> = diff
        .files
        .iter()
        .map(|f| (f.path().as_bytes(), f.change, f.hunks.len()))
        .collect();
    assert_eq!(
        summary,
        [
            (&b"conflitto\tstrano.txt"[..], Change::Unmerged, 0),
            (b"conflitto.txt", Change::Unmerged, 0),
            (b"nuovo.txt", Change::Modified, 1),
        ]
    );
    let conflitto = &diff.files[0];
    assert_eq!(conflitto.old_path, conflitto.new_path);
    assert_eq!((conflitto.old_mode, conflitto.old_oid), (None, None));

    let indice = parse(CONFLITTO_INDICE);
    let paths: Vec<(&[u8], Change)> = indice
        .files
        .iter()
        .map(|f| (f.path().as_bytes(), f.change))
        .collect();
    assert_eq!(
        paths,
        [
            (&b"conflitto\tstrano.txt"[..], Change::Unmerged),
            (b"conflitto.txt", Change::Unmerged),
        ]
    );
    assert_eq!(indice.to_patch(), CONFLITTO_INDICE);
}

#[test]
fn unmerged_con_a_capo_nel_nome() {
    let diff = parse(b"* Unmerged path a\nb\n* Unmerged path c\n");
    let paths: Vec<&[u8]> = diff.files.iter().map(|f| f.path().as_bytes()).collect();
    assert_eq!(paths, [&b"a\nb"[..], b"c"]);
}

#[test]
fn file_vuoti() {
    // File nuovo vuoto (`git add` di un file vuoto, senza altri file simili da cui una rinomina):
    // `new file mode` e `index`, niente `---`/`+++`.
    let input = format!(
        "diff --git a/vuoto b/vuoto\nnew file mode 100644\nindex {}..{EMPTY_BLOB}\n",
        ZERO
    );
    let diff = parse(input.as_bytes());
    let [vuoto] = &diff.files[..] else {
        panic!("un file atteso")
    };
    assert_eq!(vuoto.change, Change::Added);
    assert_eq!(vuoto.path().as_bytes(), b"vuoto");
    assert_eq!(
        vuoto.new_oid,
        Some(Oid::from_hex(EMPTY_BLOB.as_bytes()).unwrap())
    );
    assert!(vuoto.hunks.is_empty() && !vuoto.binary);
    assert_eq!(diff.to_patch(), input.as_bytes());
}

/// Per gli output senza conflitti né configurazione particolare, [`Diff::to_patch`] riscrive
/// esattamente i byte di git.
#[test]
fn riscrittura_identica() {
    for (name, input) in [
        ("testo", TESTO),
        ("file", FILE),
        ("copie", COPIE),
        ("nomi", NOMI),
        ("sottomodulo-nuovo", SOTTOMODULO_NUOVO),
        ("sottomodulo", SOTTOMODULO),
        ("vuoto", VUOTO),
    ] {
        let out = parse(input).to_patch();
        assert!(
            out == input,
            "{name}: riscritto diverso\n{}\n---\n{}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(input)
        );
    }
}

#[test]
fn intestazione_e_patch_di_un_file() {
    let diff = parse(TESTO);
    let f = file(&diff, b"una-riga.txt");
    let mut header = Vec::new();
    f.write_header(&mut header);
    assert!(header.starts_with(b"diff --git a/una-riga.txt b/una-riga.txt\nindex "));
    assert!(header.ends_with(b" 100644\n--- a/una-riga.txt\n+++ b/una-riga.txt\n"));
    assert_eq!(f.hunks[0].header(), b"@@ -1 +1 @@");
    let mut patch = header.clone();
    f.hunks[0].write_to(&mut patch);
    assert_eq!(patch, f.to_patch());
    assert!(patch.ends_with(b"@@ -1 +1 @@\n-solo\n+sola\n"));
}

#[test]
fn json_con_byte_non_utf8() {
    let diff = parse(TESTO);
    let latin1 = file(&diff, b"latin1.txt");
    let json = serde_json::to_value(latin1).unwrap();
    assert_eq!(json["old_path"], "latin1.txt");
    assert_eq!(json["change"], "Modified");
    assert_eq!(json["old_mode"], 0o100644);
    assert_eq!(json["old_oid"], "71489e5c1da5bd1f5d55fa4bced134f7e2b05190");
    assert_eq!(json["binary"], false);
    let hunk = &json["hunks"][0];
    assert_eq!(hunk["old_start"], 1);
    assert_eq!(hunk["section"], "");
    assert_eq!(
        hunk["lines"][0],
        serde_json::json!({
            "kind": "Context",
            "old_line": 1,
            "new_line": 1,
            "content": [99, 97, 102, 102, 0xe8],
            "no_newline": false,
        })
    );
    assert_eq!(hunk["lines"][1]["content"], "seconda");
    assert_eq!(hunk["lines"][1]["new_line"], serde_json::Value::Null);

    for input in [TESTO, FILE, NOMI, CONFLITTO] {
        let diff = parse(input);
        let back: Diff = serde_json::from_str(&serde_json::to_string(&diff).unwrap()).unwrap();
        assert_eq!(back, diff);
    }
}

#[test]
fn errori_su_input_malformato() {
    let ok = "diff --git a/x b/x\nindex 0123456789012345678901234567890123456789..\
              1123456789012345678901234567890123456789 100644\n--- a/x\n+++ b/x\n";
    let cases: Vec<(String, &str)> = vec![
        ("diff --git a/x b/x".into(), "non termina con un a capo"),
        ("ciao\n".into(), "riga n. 1: `ciao` inattesa"),
        (
            format!("{ok}@@ -1,2 +1 @@\n-a\n"),
            "hunk troncato: mancano 1 righe",
        ),
        (
            format!("{ok}@@ -1 +1 @@\n-a\n+b\n+c\n"),
            "riga n. 8: `+c` inattesa dopo",
        ),
        (
            format!("{ok}@@ -1 +1 @@\n-a\n?b\n"),
            "riga n. 7: `?b` non è una riga di hunk",
        ),
        (
            format!("{ok}@@ -1 +0,0 @@\n+a\n"),
            "più righe di quelle dichiarate",
        ),
        (
            format!("{ok}@@ -x +1 @@\n"),
            "non è un'intestazione di hunk",
        ),
        (
            format!("{ok}@@ -1 +1 @@x\n-a\n+b\n"),
            "non è un'intestazione di hunk",
        ),
        (
            format!("{ok}@@ -1 +1 @@\n\\ No newline\n"),
            "senza una riga prima",
        ),
        (
            format!("{ok}diff --git a/y b/y\n"),
            "riga n. 5: nessun hunk dopo la riga `+++`",
        ),
        (
            "diff --git a/x b/x\n--- a/x\nciao\n".into(),
            "riga n. 3: manca la riga `+++`",
        ),
        (
            "diff --git x x\n--- x\n+++ x\n@@ -1 +1 @@\n-a\n+b\n".into(),
            "non inizia con `a/`",
        ),
        (
            "diff --git a/x b/x\nindex abc..def 100644\n".into(),
            "`--full-index`",
        ),
        (
            "diff --git a/x b/x\nold mode 10064x\n".into(),
            "non è un modo ottale valido",
        ),
        (
            "diff --git a/x b/x\nsimilarity index 101%\n".into(),
            "non è una percentuale valida",
        ),
        (
            "diff --git a/x b/x\nrename from x\n".into(),
            "senza entrambe le righe",
        ),
        (
            "diff --git a/x b/x\nboh\n".into(),
            "riga n. 2: intestazione `boh` sconosciuta",
        ),
        (
            "diff --git a/x b/x\n@@ -1 +1 @@\n".into(),
            "hunk senza le righe `---`",
        ),
        (
            "diff --git a/x b/x\nGIT binary patch\n".into(),
            "patch binaria",
        ),
        (
            "diff --git a/x b/x\ndissimilarity index 90%\n".into(),
            "`-B`",
        ),
        (
            "diff --git \"a/x\\q\" b/x\n".into(),
            "escape `\\q` sconosciuto",
        ),
        (
            "diff --git \"a/x\\04\" b/x\n".into(),
            "escape ottale non valido",
        ),
        ("diff --git \"a/x b/x\n".into(), "virgolette non chiuse"),
        (
            "diff --git a/x c b/y d b/z\n".into(),
            "impossibile ricavare i percorsi",
        ),
        (
            "diff --git a/x b/x\nnew file mode 100644\ndeleted file mode 100644\n".into(),
            "tipi di modifica diversi",
        ),
        (
            "diff --git a/x b/x\n--- /dev/null\n+++ /dev/null\n".into(),
            "entrambi `/dev/null`",
        ),
        ("diff --cc \"x\n".into(), "virgolette non chiuse"),
    ];
    for (input, expected) in cases {
        let r = reason(input.as_bytes());
        assert!(
            r.contains(expected),
            "{input:?}: `{r}` non contiene `{expected}`"
        );
    }
}

#[test]
fn virgolette_decodificate() {
    use super::quote::{unquote, write_quoted};
    assert_eq!(
        unquote(br#""a\tb\n\"\\\a\b\v\f\r\001\377""#).unwrap(),
        b"a\tb\n\"\\\x07\x08\x0b\x0c\r\x01\xff"
    );
    assert_eq!(unquote(b"senza virgolette").unwrap(), b"senza virgolette");
    assert!(
        unquote(br#""a" b"#)
            .unwrap_err()
            .contains("dopo le virgolette")
    );
    let mut out = Vec::new();
    write_quoted(&mut out, b"a/", b"x\x01\x7f\t\xff y");
    assert_eq!(
        out,
        br#""a/x\001\177\t"#.iter().copied().chain(*b"\xff y\"").collect::<Vec<u8>>()
    );
}

fn path_strategy() -> impl Strategy<Value = RepoPath> {
    proptest::collection::vec(1u8..=255, 1..12).prop_map(RepoPath::from)
}

fn content_strategy() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(
        any::<u8>().prop_filter("niente a capo", |b| *b != b'\n'),
        0..10,
    )
}

fn hunk_strategy() -> impl Strategy<Value = Hunk> {
    let line = (0u8..3, content_strategy(), any::<bool>());
    (
        0u32..1000,
        0u32..1000,
        content_strategy(),
        proptest::collection::vec(line, 0..8),
    )
        .prop_map(|(old_start, new_start, section, raw)| {
            let (mut old_no, mut new_no) = (old_start, new_start);
            let lines: Vec<Line> = raw
                .into_iter()
                .map(|(k, content, no_newline)| {
                    let kind = [LineKind::Context, LineKind::Added, LineKind::Removed][k as usize];
                    let old_line = (kind != LineKind::Added).then(|| {
                        old_no += 1;
                        old_no - 1
                    });
                    let new_line = (kind != LineKind::Removed).then(|| {
                        new_no += 1;
                        new_no - 1
                    });
                    Line {
                        kind,
                        old_line,
                        new_line,
                        content: ByteText::from(content),
                        no_newline,
                    }
                })
                .collect();
            Hunk {
                old_start,
                old_count: old_no - old_start,
                new_start,
                new_count: new_no - new_start,
                section: ByteText::from(section),
                lines,
            }
        })
}

/// Un [`FileDiff`] coerente, come potrebbe produrlo git.
fn file_strategy() -> impl Strategy<Value = FileDiff> {
    let modes = prop_oneof![
        Just(0o100644u32),
        Just(0o100755),
        Just(0o120000),
        Just(0o160000)
    ];
    let oid = proptest::array::uniform20(any::<u8>()).prop_map(Oid::from_bytes);
    (
        0u8..6,
        path_strategy(),
        path_strategy(),
        0u8..=100,
        (modes.clone(), modes, 0u8..3),
        proptest::option::of((oid.clone(), oid)),
        any::<bool>(),
        proptest::collection::vec(hunk_strategy(), 1..3),
    )
        .prop_map(
            |(kind, old, new, score, (m1, m2, mode_kind), oids, binary, hunks)| {
                let change = [
                    Change::Modified,
                    Change::Added,
                    Change::Deleted,
                    Change::Renamed,
                    Change::Copied,
                    Change::Unmerged,
                ][kind as usize];
                let mut f = FileDiff {
                    old_path: old.clone(),
                    new_path: new,
                    change,
                    similarity: None,
                    old_mode: None,
                    new_mode: None,
                    old_oid: oids.map(|o| o.0),
                    new_oid: oids.map(|o| o.1),
                    binary,
                    hunks: if binary { Vec::new() } else { hunks },
                };
                match change {
                    Change::Added => f.new_mode = Some(m1),
                    Change::Deleted => f.old_mode = Some(m1),
                    // Modi uguali solo se c'è la riga `index` che li riporta.
                    _ => match mode_kind {
                        0 => {}
                        1 if oids.is_some() => (f.old_mode, f.new_mode) = (Some(m1), Some(m1)),
                        _ if m1 != m2 => (f.old_mode, f.new_mode) = (Some(m1), Some(m2)),
                        _ => {}
                    },
                }
                if matches!(change, Change::Renamed | Change::Copied) {
                    f.similarity = (score % 2 == 0).then_some(score);
                } else {
                    f.new_path = old;
                }
                if change == Change::Unmerged {
                    let path: Vec<u8> = f
                        .old_path
                        .as_bytes()
                        .iter()
                        .map(|&b| if b == b'\n' { b'_' } else { b })
                        .collect();
                    f = super::diff::unmerged_file(RepoPath::from(path));
                }
                f
            },
        )
}

proptest! {
    /// Nessun input fa andare in panico il parser.
    #[test]
    fn mai_panico(input in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = parse_diff(&input);
    }

    /// Nessun panico neanche su righe che assomigliano a un diff, mescolate a caso.
    #[test]
    fn mai_panico_su_righe_da_diff(
        picks in proptest::collection::vec((0usize..16, content_strategy()), 0..30)
    ) {
        const PIECES: [&[u8]; 16] = [
            b"diff --git a/x b/x", b"diff --git \"a/", b"--- a/x", b"+++ b/x", b"--- /dev/null",
            b"@@ -1 +1 @@", b"@@ -0,0 +1,2 @@ f", b"@@ -4294967295,2 +1 @@", b"-", b"+", b" ",
            b"\\ No newline at end of file", b"index ", b"new file mode 100644",
            b"rename from ", b"* Unmerged path ",
        ];
        let mut input = Vec::new();
        for (i, tail) in picks {
            input.extend_from_slice(PIECES[i]);
            input.extend_from_slice(&tail);
            input.push(b'\n');
        }
        let _ = parse_diff(&input);
    }

    /// Un diff coerente riscritto con [`Diff::to_patch`] si rilegge identico, con nomi e
    /// contenuti arbitrari (virgolette, a capo, byte non UTF-8).
    #[test]
    fn riscrittura_e_rilettura(files in proptest::collection::vec(file_strategy(), 0..4)) {
        let diff = Diff { files };
        let patch = diff.to_patch();
        let back = parse_diff(&patch);
        prop_assert_eq!(back.as_ref(), Ok(&diff), "{}", String::from_utf8_lossy(&patch));
    }
}
