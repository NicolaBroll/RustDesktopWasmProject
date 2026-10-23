use super::*;
use crate::DomainError;
use proptest::prelude::*;

// File del worktree in conflitto, reali, generati con git 2.56.0 da `testdata/conflict.sh`
// (`sh conflict.sh testdata`): `git merge altro` in un repo temporaneo con
// `GIT_CONFIG_GLOBAL=/dev/null`, poi `git checkout --conflict=diff3|zdiff3 -- merge.txt`.

/// Due blocchi, stile merge, etichette `HEAD` e `altro`.
const MERGE: &[u8] = include_bytes!("testdata/conflict-merge.bin");
/// Lo stesso file con `--conflict=diff3`: etichette `ours`, `base`, `theirs`.
const DIFF3: &[u8] = include_bytes!("testdata/conflict-diff3.bin");
/// Lo stesso con `--conflict=zdiff3`.
const ZDIFF3: &[u8] = include_bytes!("testdata/conflict-zdiff3.bin");
/// Righe con CRLF.
const CRLF: &[u8] = include_bytes!("testdata/conflict-crlf.bin");
/// Ultima riga senza a capo da entrambi i lati (git aggiunge l'a capo nel blocco).
const CODA: &[u8] = include_bytes!("testdata/conflict-coda.bin");
/// Il lato nostro ha tolto le righe che il loro cambia: `ours` vuoto.
const VUOTO: &[u8] = include_bytes!("testdata/conflict-vuoto.bin");
/// `conflict-marker-size=10` da `.gitattributes`.
const SIZE: &[u8] = include_bytes!("testdata/conflict-size.bin");
/// Contenuto non UTF-8 (latin-1).
const LATIN1: &[u8] = include_bytes!("testdata/conflict-latin1.bin");
/// Lo script che genera i file.
const SCRIPT: &str = include_str!("testdata/conflict.sh");

fn parse(text: &[u8]) -> ConflictText {
    parse_conflicts(text, 7).expect("conflitto valido")
}

fn blocks(parsed: &ConflictText) -> Vec<&ConflictBlock> {
    parsed.blocks().collect()
}

fn common(bytes: &[u8]) -> Segment {
    Segment::Common(bytes.to_vec())
}

/// Ricompone il file: segmenti comuni e, per i blocchi, i byte del testo nel loro `range`.
fn rebuild(text: &[u8], parsed: &ConflictText) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0;
    for s in &parsed.segments {
        match s {
            Segment::Common(bytes) => {
                assert!(!bytes.is_empty(), "segmento comune vuoto");
                assert_eq!(&text[pos..pos + bytes.len()], bytes.as_slice());
                pos += bytes.len();
                out.extend_from_slice(bytes);
            }
            Segment::Conflict(b) => {
                assert_eq!(b.range.start, pos, "il blocco segue il segmento precedente");
                out.extend_from_slice(&text[b.range.clone()]);
                pos = b.range.end;
            }
        }
    }
    out
}

fn invalid(result: Result<impl std::fmt::Debug, DomainError>) -> String {
    match result {
        Err(DomainError::InvalidConflict { reason }) => reason,
        other => panic!("atteso InvalidConflict, ottenuto {other:?}"),
    }
}

#[test]
fn lo_script_genera_i_file_dei_test() {
    for f in [
        "merge", "diff3", "zdiff3", "crlf", "coda", "vuoto", "size", "latin1",
    ] {
        assert!(SCRIPT.contains(&format!("conflict-{f}.bin")), "{f}");
    }
}

#[test]
fn stile_merge() {
    let parsed = parse(MERGE);
    assert_eq!(parsed.block_count(), 2);
    assert_eq!(parsed.segments.len(), 5);
    assert_eq!(parsed.segments[0], common(b"uno\n"));
    assert_eq!(
        parsed.segments[2],
        common(b"tre\nquattro\ncinque\nsei\nsette\n")
    );
    assert_eq!(parsed.segments[4], common(b"nove\n"));
    let b = blocks(&parsed);
    assert_eq!(b[0].ours, b"DUE main\n");
    assert_eq!(b[0].theirs, b"DUE altro\n");
    assert_eq!(b[0].base, None);
    assert_eq!(b[0].ours_label, b"HEAD");
    assert_eq!(b[0].theirs_label, b"altro");
    assert_eq!(b[0].base_label, b"");
    assert_eq!(b[1].ours, b"OTTO main\nOTTO bis\n");
    assert_eq!(b[1].theirs, b"OTTO altro\n");
    assert_eq!(rebuild(MERGE, &parsed), MERGE);
}

#[test]
fn stile_diff3_e_zdiff3() {
    for text in [DIFF3, ZDIFF3] {
        let parsed = parse(text);
        let b = blocks(&parsed);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].ours, b"DUE main\n");
        assert_eq!(b[0].base.as_deref(), Some(&b"due\n"[..]));
        assert_eq!(b[0].theirs, b"DUE altro\n");
        assert_eq!(
            (
                b[0].ours_label.as_slice(),
                b[0].base_label.as_slice(),
                b[0].theirs_label.as_slice()
            ),
            (&b"ours"[..], &b"base"[..], &b"theirs"[..])
        );
        assert_eq!(b[1].base.as_deref(), Some(&b"otto\n"[..]));
        assert_eq!(rebuild(text, &parsed), text);
    }
}

#[test]
fn range_delimita_esattamente_il_blocco() {
    for text in [MERGE, DIFF3, CRLF, CODA, VUOTO, LATIN1] {
        let parsed = parse(text);
        for b in parsed.blocks() {
            let bytes = &text[b.range.clone()];
            assert!(bytes.starts_with(b"<<<<<<< "), "{bytes:?}");
            assert!(bytes.ends_with(b"\n"));
            let last = bytes[..bytes.len() - 1]
                .rsplit(|&c| c == b'\n')
                .next()
                .unwrap_or_default();
            assert!(last.starts_with(b">>>>>>> "), "{last:?}");
            // Il blocco inizia a inizio riga.
            assert!(b.range.start == 0 || text[b.range.start - 1] == b'\n');
        }
    }
    let parsed = parse(MERGE);
    assert_eq!(blocks(&parsed)[0].range, 4..58);
    assert_eq!(
        &MERGE[4..58],
        b"<<<<<<< HEAD\nDUE main\n=======\nDUE altro\n>>>>>>> altro\n"
    );
}

#[test]
fn crlf_conservato() {
    let parsed = parse(CRLF);
    let b = blocks(&parsed);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].ours, b"DUE main\r\n");
    assert_eq!(b[0].theirs, b"DUE altro\r\n");
    // Le etichette non hanno il `\r`.
    assert_eq!(b[0].ours_label, b"HEAD");
    assert_eq!(b[0].theirs_label, b"altro");
    assert_eq!(parsed.segments[0], common(b"uno\r\n"));
    assert_eq!(
        resolve_all(CRLF, &parsed, Choice::Theirs).unwrap(),
        b"uno\r\nDUE altro\r\ntre\r\n"
    );
}

#[test]
fn ultima_riga_senza_a_capo() {
    // Reale: git aggiunge l'a capo alle righe senza a capo dentro il blocco.
    let parsed = parse(CODA);
    let b = blocks(&parsed);
    assert_eq!(b[0].ours, b"due main\n");
    assert_eq!(b[0].range.end, CODA.len());
    // Testo comune finale senza a capo.
    let text = b"<<<<<<< a\nx\n=======\ny\n>>>>>>> b\nfine";
    let parsed = parse(text);
    assert_eq!(parsed.segments.last(), Some(&common(b"fine")));
    assert_eq!(
        resolve_all(text, &parsed, Choice::Ours).unwrap(),
        b"x\nfine"
    );
    // Marcatore di chiusura come ultima riga senza a capo.
    let text = b"a\n<<<<<<< a\nx\n=======\ny\n>>>>>>> b";
    let parsed = parse(text);
    let b = blocks(&parsed);
    assert_eq!(b[0].range, 2..text.len());
    assert_eq!(b[0].theirs_label, b"b");
    assert_eq!(parsed.segments.len(), 2);
    assert_eq!(
        resolve_all(text, &parsed, Choice::Theirs).unwrap(),
        b"a\ny\n"
    );
}

#[test]
fn blocco_vuoto_da_un_lato() {
    let parsed = parse(VUOTO);
    let b = blocks(&parsed);
    assert_eq!(b[0].ours, b"");
    assert_eq!(b[0].theirs, b"X\nY\n");
    assert_eq!(
        resolve_all(VUOTO, &parsed, Choice::Ours).unwrap(),
        b"prima\ndopo\n"
    );
    // Entrambi i lati vuoti, senza etichette.
    let text = b"<<<<<<<\n=======\n>>>>>>>\n";
    let parsed = parse(text);
    let b = blocks(&parsed);
    assert_eq!((b[0].ours.len(), b[0].theirs.len()), (0, 0));
    assert_eq!(b[0].ours_label, b"");
    assert_eq!(b[0].theirs_label, b"");
    assert_eq!(
        resolve_all(text, &parsed, Choice::OursThenTheirs).unwrap(),
        b""
    );
}

#[test]
fn marker_size_diverso() {
    // Con la lunghezza giusta i marcatori da 10 sono un blocco.
    let parsed = parse_conflicts(SIZE, 10).unwrap();
    let b = blocks(&parsed);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].ours, b"DUE main\n");
    assert_eq!(b[0].theirs_label, b"altro");
    // Con 7 sono righe più lunghe del marcatore: testo.
    let parsed = parse(SIZE);
    assert_eq!(parsed.segments, vec![common(SIZE)]);
    // Con 10, i marcatori da 7 dei file normali sono testo.
    assert_eq!(
        parse_conflicts(MERGE, 10).unwrap().segments,
        vec![common(MERGE)]
    );
    assert!(invalid(parse_conflicts(MERGE, 0)).contains("almeno 1"));
}

#[test]
fn marcatori_piu_lunghi_o_attaccati_sono_testo() {
    let text = b"<<<<<<<< no\n<<<<<<<x\n=======\n>>>>>>>>\n";
    assert_eq!(parse(text).segments, vec![common(text)]);
    // Dentro un blocco una riga `========` è contenuto.
    let text = b"<<<<<<< a\n========\n=======\n>>>>>>>>\n>>>>>>> b\n";
    let parsed = parse(text);
    let b = blocks(&parsed);
    assert_eq!(b[0].ours, b"========\n");
    assert_eq!(b[0].theirs, b">>>>>>>>\n");
}

#[test]
fn separatore_isolato_e_testo() {
    // Titolo setext e marcatori di mezzo senza `<<<<<<<` aperto.
    let text = b"Titolo\n=======\n|||||||\n>>>>>>> x\ntesto\n";
    assert_eq!(parse(text).segments, vec![common(text)]);
    // Anche dopo un blocco chiuso.
    let text = b"<<<<<<< a\nx\n=======\ny\n>>>>>>> b\nTitolo\n=======\n";
    let parsed = parse(text);
    assert_eq!(parsed.block_count(), 1);
    assert_eq!(parsed.segments[1], common(b"Titolo\n=======\n"));
}

#[test]
fn blocco_non_chiuso() {
    let reason = invalid(parse_conflicts(b"a\n<<<<<<< HEAD\nx\n=======\ny\n", 7));
    assert!(reason.contains("riga 2"), "{reason}");
    assert!(reason.contains("non è chiuso"), "{reason}");
    // Il file troncato dopo il primo blocco reale.
    let parsed = parse(MERGE);
    let end = blocks(&parsed)[1].range.end - 1 - b">>>>>>> altro".len();
    assert!(parse_conflicts(&MERGE[..end], 7).is_err());
}

#[test]
fn parti_nell_ordine_sbagliato() {
    for (text, riga) in [
        (&b"<<<<<<< a\nx\n>>>>>>> b\n"[..], "riga 3"),
        (b"<<<<<<< a\n<<<<<<< b\n", "riga 2"),
        (b"<<<<<<< a\n=======\n||||||| base\n>>>>>>> b\n", "riga 3"),
        (b"<<<<<<< a\n||||||| base\n||||||| base\n", "riga 3"),
        (b"<<<<<<< a\n=======\n=======\n>>>>>>> b\n", "riga 3"),
        (b"<<<<<<< a\n||||||| o\n>>>>>>> b\n", "riga 3"),
    ] {
        let reason = invalid(parse_conflicts(text, 7));
        assert!(reason.contains(riga), "{reason}");
        assert!(reason.contains("riga 1"), "{reason}");
    }
}

#[test]
fn non_utf8_senza_perdite() {
    let parsed = parse(LATIN1);
    let b = blocks(&parsed);
    assert_eq!(b[0].ours, b"caff\xe8 main\n");
    assert_eq!(b[0].theirs, b"caff\xe8 altro\n");
    assert_eq!(
        resolve_all(LATIN1, &parsed, Choice::TheirsThenOurs).unwrap(),
        b"caff\xe8 altro\ncaff\xe8 main\ndue\n"
    );
    // Etichetta non UTF-8.
    let text = b"<<<<<<< r\xe9f\n=======\n>>>>>>> \xff\n";
    let b = parse(text);
    let b = blocks(&b);
    assert_eq!(b[0].ours_label, b"r\xe9f");
    assert_eq!(b[0].theirs_label, b"\xff");
}

#[test]
fn testo_senza_conflitti() {
    assert_eq!(parse(b"").segments, vec![]);
    assert_eq!(parse(b"a\nb").segments, vec![common(b"a\nb")]);
    assert_eq!(
        resolve_all(b"a\nb", &parse(b"a\nb"), Choice::Ours).unwrap(),
        b"a\nb"
    );
}

#[test]
fn resolved_per_ogni_scelta() {
    let parsed = parse(DIFF3);
    let b = blocks(&parsed)[1];
    let cases = [
        (Choice::Ours, &b"OTTO main\nOTTO bis\n"[..]),
        (Choice::Theirs, b"OTTO altro\n"),
        (Choice::Base, b"otto\n"),
        (Choice::OursThenTheirs, b"OTTO main\nOTTO bis\nOTTO altro\n"),
        (Choice::TheirsThenOurs, b"OTTO altro\nOTTO main\nOTTO bis\n"),
    ];
    for (choice, expected) in cases {
        assert_eq!(b.resolved(choice).unwrap(), expected, "{choice:?}");
    }
    // Senza base, `Base` è un errore.
    let parsed = parse(MERGE);
    let reason = invalid(blocks(&parsed)[0].resolved(Choice::Base));
    assert!(reason.contains("base"), "{reason}");
}

#[test]
fn resolve_block_per_ogni_scelta() {
    let parsed = parse(DIFF3);
    let head = b"uno\n".as_slice();
    let mid = b"tre\nquattro\ncinque\nsei\nsette\n".as_slice();
    let second = &DIFF3[blocks(&parsed)[1].range.clone()];
    let cases = [
        (Choice::Ours, &b"DUE main\n"[..]),
        (Choice::Theirs, b"DUE altro\n"),
        (Choice::Base, b"due\n"),
        (Choice::OursThenTheirs, b"DUE main\nDUE altro\n"),
        (Choice::TheirsThenOurs, b"DUE altro\nDUE main\n"),
    ];
    for (choice, resolved) in cases {
        let out = resolve_block(DIFF3, &parsed, 0, choice).unwrap();
        // Solo il primo blocco cambia: il secondo resta con i marcatori.
        let expected = [head, resolved, mid, second, b"nove\n"].concat();
        assert_eq!(out, expected, "{choice:?}");
        // Il risultato ha ancora un blocco, uguale al secondo.
        let again = parse(&out);
        assert_eq!(again.block_count(), 1);
        assert_eq!(blocks(&again)[0].ours, blocks(&parsed)[1].ours);
    }
    // Il secondo blocco, lasciando il primo.
    let out = resolve_block(MERGE, &parse(MERGE), 1, Choice::Theirs).unwrap();
    let first = &MERGE[blocks(&parse(MERGE))[0].range.clone()];
    assert_eq!(
        out,
        [b"uno\n", first, mid, b"OTTO altro\n", b"nove\n"].concat()
    );
    // Indice inesistente.
    let reason = invalid(resolve_block(MERGE, &parse(MERGE), 2, Choice::Ours));
    assert!(reason.contains("2 blocchi"), "{reason}");
    // `Base` senza base.
    assert!(resolve_block(MERGE, &parse(MERGE), 0, Choice::Base).is_err());
    // Testo diverso da quello letto.
    assert!(resolve_block(b"corto", &parse(MERGE), 1, Choice::Ours).is_err());
}

#[test]
fn resolve_all_per_ogni_scelta() {
    let parsed = parse(DIFF3);
    let cases = [
        (Choice::Ours, "DUE main\n", "OTTO main\nOTTO bis\n"),
        (Choice::Theirs, "DUE altro\n", "OTTO altro\n"),
        (Choice::Base, "due\n", "otto\n"),
        (
            Choice::OursThenTheirs,
            "DUE main\nDUE altro\n",
            "OTTO main\nOTTO bis\nOTTO altro\n",
        ),
        (
            Choice::TheirsThenOurs,
            "DUE altro\nDUE main\n",
            "OTTO altro\nOTTO main\nOTTO bis\n",
        ),
    ];
    for (choice, first, second) in cases {
        let expected = format!("uno\n{first}tre\nquattro\ncinque\nsei\nsette\n{second}nove\n");
        assert_eq!(
            resolve_all(DIFF3, &parsed, choice).unwrap(),
            expected.as_bytes(),
            "{choice:?}"
        );
    }
    // Stile merge: `Base` non c'è.
    assert!(resolve_all(MERGE, &parse(MERGE), Choice::Base).is_err());
    assert_eq!(
        resolve_all(MERGE, &parse(MERGE), Choice::Ours).unwrap(),
        b"uno\nDUE main\ntre\nquattro\ncinque\nsei\nsette\nOTTO main\nOTTO bis\nnove\n"
    );
}

#[test]
fn choice_in_json() {
    let json = serde_json::to_string(&Choice::OursThenTheirs).unwrap();
    assert_eq!(json, "\"ours_then_theirs\"");
    let back: Choice = serde_json::from_str("\"theirs\"").unwrap();
    assert_eq!(back, Choice::Theirs);
}

/// Una riga qualsiasi: testo, a volte con un carattere da marcatore all'inizio, con `\n`,
/// `\r\n` o (se ultima) senza a capo.
fn riga() -> impl Strategy<Value = Vec<u8>> {
    (
        prop::sample::select(vec![
            &b""[..],
            b"<",
            b"=",
            b">",
            b"|",
            b"=======",
            b"<<<<<<<<",
        ]),
        prop::collection::vec(prop::sample::select(b"ab \xe8".to_vec()), 0..4),
        prop::sample::select(vec![&b"\n"[..], b"\r\n"]),
    )
        .prop_map(|(pre, body, eol)| [pre, &body, eol].concat())
}

fn righe() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(riga(), 0..4).prop_map(|r| r.concat())
}

/// Un blocco ben formato: (testo, ours, base, theirs).
fn blocco() -> impl Strategy<Value = (Vec<u8>, Vec<u8>, Option<Vec<u8>>, Vec<u8>)> {
    // Le righe dentro un blocco non cominciano con un marcatore valido: niente `=======`.
    let dentro = || {
        righe().prop_filter("niente marcatori", |r| {
            !r.split(|&c| c == b'\n')
                .any(|l| l.starts_with(b"=======") && !l.starts_with(b"========"))
        })
    };
    (dentro(), prop::option::of(dentro()), dentro()).prop_map(|(ours, base, theirs)| {
        let mut text = b"<<<<<<< nostra\n".to_vec();
        text.extend_from_slice(&ours);
        if let Some(base) = &base {
            text.extend_from_slice(b"||||||| base\r\n");
            text.extend_from_slice(base);
        }
        text.extend_from_slice(b"=======\n");
        text.extend_from_slice(&theirs);
        text.extend_from_slice(b">>>>>>> loro\n");
        (text, ours, base, theirs)
    })
}

proptest! {
    #[test]
    fn segmenti_ricompongono_il_file(
        parts in prop::collection::vec((righe(), blocco()), 0..4),
        coda in righe(),
        senza_a_capo in any::<bool>(),
    ) {
        let mut text = Vec::new();
        let mut expected = Vec::new();
        for (comune, (blocco, ours, base, theirs)) in &parts {
            text.extend_from_slice(comune);
            text.extend_from_slice(blocco);
            expected.push((ours.clone(), base.clone(), theirs.clone()));
        }
        text.extend_from_slice(&coda);
        if senza_a_capo && text.ends_with(b"\n") {
            text.pop();
        }
        let parsed = parse_conflicts(&text, 7).unwrap();
        prop_assert_eq!(rebuild(&text, &parsed), text.clone());
        let found: Vec<_> = parsed
            .blocks()
            .map(|b| (b.ours.clone(), b.base.clone(), b.theirs.clone()))
            .collect();
        prop_assert_eq!(found.len(), expected.len());
        for (f, e) in found.iter().zip(&expected) {
            prop_assert_eq!(f, e);
        }
        // Risolvere tutto con `Ours` equivale a risolvere i blocchi uno alla volta, dall'ultimo.
        let mut one_by_one = text.clone();
        for i in (0..parsed.block_count()).rev() {
            one_by_one = resolve_block(&one_by_one, &parsed, i, Choice::Ours).unwrap();
        }
        prop_assert_eq!(resolve_all(&text, &parsed, Choice::Ours).unwrap(), one_by_one);
    }
}
