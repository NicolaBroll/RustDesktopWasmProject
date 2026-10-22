use super::refs::parse_track;
use super::*;
use crate::DomainError;
use proptest::prelude::*;

// Output reale di git 2.43.0, generato da `testdata/refs.sh` (`sh refs.sh testdata`) in repo
// temporanei con `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1` e date fisse. Il comando
// è sempre
//
// ```sh
// git for-each-ref \
//     '--format=%(refname)%00%(objectname)%00%(*objectname)%00%(upstream)%00%(upstream:track,nobracket)%00%(symref)%00' \
//     refs/heads/ refs/remotes/ refs/tags/ refs/stash
// ```
//
// cioè gli argomenti di [`REFS_ARGS`].

/// Clone di un remoto bare con: `main` in pari, `avanti` (ahead 2), `indietro` (behind 1),
/// `entrambi` (ahead 1, behind 2), `sparito` (upstream cancellato, gone), `su-main` (upstream
/// locale `refs/heads/main`), branch senza upstream `feature/nuova-interfaccia`, `caffè` e
/// `latin-\xff` (non UTF-8), il ref simbolico `refs/heads/alias`, `origin/HEAD` simbolico,
/// `origin/solo-remoto`, tag leggero `v0.1`, annotato `v1.0`, annotato di un annotato
/// `v1.0-doppio`, leggero su un albero `albero`, `refs/notes/commits` e uno stash.
const REFS: &[u8] = include_bytes!("testdata/refs.bin");
/// Repo appena creato con `git init`: zero byte.
const VUOTO: &[u8] = include_bytes!("testdata/refs-vuoto.bin");
/// `main` con `branch.main.merge` che contiene un a capo: `%(upstream)` lo riporta così com'è.
const UPSTREAM_A_CAPO: &[u8] = include_bytes!("testdata/refs-upstream-a-capo.bin");
/// Lo script che genera i file: deve usare lo stesso formato di [`REFS_FORMAT`].
const SCRIPT: &str = include_str!("testdata/refs.sh");

const BASE: &str = "2e4cc4a040054526ce0cf695148985f95d13ebd2";
const AVANTI: &str = "f4e464a67fc3d26810b646c18e22a344a7e7b124";

fn oid(hex: &str) -> Oid {
    Oid::from_hex(hex.as_bytes()).expect("oid di test valido")
}

fn parse(input: &[u8]) -> Vec<RefInfo> {
    parse_refs(input).expect("output reale valido")
}

fn reason(err: DomainError) -> String {
    match err {
        DomainError::InvalidGitOutput { command, reason } => {
            assert_eq!(command, "for-each-ref");
            reason
        }
        other => panic!("errore inatteso: {other:?}"),
    }
}

fn find<'a>(refs: &'a [RefInfo], name: &str) -> &'a RefInfo {
    refs.iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("manca {name}"))
}

fn branch(name: &str, target: &str, upstream: Option<(&str, Option<u32>, Option<u32>)>) -> RefInfo {
    RefInfo {
        name: name.into(),
        kind: RefKind::LocalBranch,
        target: oid(target),
        tag_object: None,
        upstream: upstream.map(|(name, ahead, behind)| Upstream {
            name: name.into(),
            ahead,
            behind,
        }),
    }
}

fn plain(name: &str, kind: RefKind, target: &str) -> RefInfo {
    RefInfo {
        name: name.into(),
        kind,
        target: oid(target),
        tag_object: None,
        upstream: None,
    }
}

#[test]
fn lo_script_usa_il_formato_della_costante() {
    assert!(SCRIPT.contains(&format!("'--format={REFS_FORMAT}'")));
    assert_eq!(REFS_ARGS[0], format!("--format={REFS_FORMAT}"));
    assert!(SCRIPT.contains("refs/heads/ refs/remotes/ refs/tags/ refs/stash"));
    assert_eq!(
        &REFS_ARGS[1..],
        ["refs/heads/", "refs/remotes/", "refs/tags/", "refs/stash"]
    );
    assert_eq!(REFS_FORMAT.matches("%00").count(), REFS_FIELDS);
}

#[test]
fn nomi_e_tipi_nell_ordine_di_git() {
    let refs = parse(REFS);
    let names: Vec<(&str, RefKind)> = refs.iter().map(|r| (r.name.as_str(), r.kind)).collect();
    use RefKind::*;
    assert_eq!(
        names,
        [
            ("refs/heads/avanti", LocalBranch),
            ("refs/heads/caff\u{e8}", LocalBranch),
            ("refs/heads/entrambi", LocalBranch),
            ("refs/heads/feature/nuova-interfaccia", LocalBranch),
            ("refs/heads/indietro", LocalBranch),
            ("refs/heads/main", LocalBranch),
            ("refs/heads/sparito", LocalBranch),
            ("refs/heads/su-main", LocalBranch),
            ("refs/remotes/origin/avanti", RemoteBranch),
            ("refs/remotes/origin/entrambi", RemoteBranch),
            ("refs/remotes/origin/indietro", RemoteBranch),
            ("refs/remotes/origin/main", RemoteBranch),
            ("refs/remotes/origin/solo-remoto", RemoteBranch),
            ("refs/stash", Stash),
            ("refs/tags/albero", Tag),
            ("refs/tags/v0.1", Tag),
            ("refs/tags/v1.0", Tag),
            ("refs/tags/v1.0-doppio", Tag),
        ]
    );
}

#[test]
fn ignorati_simbolici_e_non_utf8() {
    let refs = parse(REFS);
    // Nell'output ci sono, ma il parser li scarta.
    for raw in [
        &b"refs/heads/alias\0"[..],
        b"refs/remotes/origin/HEAD\0",
        b"refs/heads/latin-\xff\0",
    ] {
        assert!(REFS.windows(raw.len()).any(|w| w == raw));
    }
    assert!(!refs.iter().any(|r| r.name == "refs/heads/alias"));
    assert!(!refs.iter().any(|r| r.name.ends_with("/HEAD")));
    assert!(!refs.iter().any(|r| r.name.starts_with("refs/heads/latin-")));
    // I pattern escludono le note.
    assert!(!REFS.windows(11).any(|w| w == b"refs/notes/"));
}

#[test]
fn branch_locali_con_upstream() {
    let refs = parse(REFS);
    let origin = |b: &str| format!("refs/remotes/origin/{b}");
    let cases = [
        branch(
            "refs/heads/main",
            BASE,
            Some((&origin("main"), Some(0), Some(0))),
        ),
        branch(
            "refs/heads/avanti",
            AVANTI,
            Some((&origin("avanti"), Some(2), Some(0))),
        ),
        branch(
            "refs/heads/indietro",
            BASE,
            Some((&origin("indietro"), Some(0), Some(1))),
        ),
        branch(
            "refs/heads/entrambi",
            "48c6fdcb9239a5c50cbf6742852bab95a4dc8c05",
            Some((&origin("entrambi"), Some(1), Some(2))),
        ),
        branch(
            "refs/heads/sparito",
            BASE,
            Some((&origin("sparito"), None, None)),
        ),
        branch(
            "refs/heads/su-main",
            BASE,
            Some(("refs/heads/main", Some(0), Some(0))),
        ),
        branch("refs/heads/feature/nuova-interfaccia", BASE, None),
        branch("refs/heads/caff\u{e8}", BASE, None),
    ];
    for expected in cases {
        assert_eq!(find(&refs, &expected.name), &expected);
    }
    assert_eq!(
        find(&refs, "refs/heads/feature/nuova-interfaccia").short_name(),
        "feature/nuova-interfaccia"
    );
    assert_eq!(
        find(&refs, "refs/heads/caff\u{e8}").short_name(),
        "caff\u{e8}"
    );
}

#[test]
fn remoti_e_stash() {
    let refs = parse(REFS);
    for expected in [
        plain("refs/remotes/origin/main", RefKind::RemoteBranch, BASE),
        plain("refs/remotes/origin/avanti", RefKind::RemoteBranch, BASE),
        plain(
            "refs/remotes/origin/entrambi",
            RefKind::RemoteBranch,
            "9c1c833001199d18895aecc781c0303dae6a5d02",
        ),
        plain(
            "refs/remotes/origin/indietro",
            RefKind::RemoteBranch,
            "19fd97318570cc11bce4bf218eb6c1952877d463",
        ),
        plain(
            "refs/remotes/origin/solo-remoto",
            RefKind::RemoteBranch,
            "06c7ac9b0a3e6e2832699b765259a30e4a5da580",
        ),
        plain(
            "refs/stash",
            RefKind::Stash,
            "708adcc764adadafc6e7cc12295a02e10afd3c38",
        ),
    ] {
        assert_eq!(find(&refs, &expected.name), &expected);
    }
    assert_eq!(
        find(&refs, "refs/remotes/origin/solo-remoto").short_name(),
        "origin/solo-remoto"
    );
}

#[test]
fn tag_leggeri_e_annotati() {
    let refs = parse(REFS);
    assert_eq!(
        find(&refs, "refs/tags/v0.1"),
        &plain("refs/tags/v0.1", RefKind::Tag, BASE)
    );
    // Annotato: target è il commit, tag_object l'oggetto tag.
    let v1 = find(&refs, "refs/tags/v1.0");
    assert_eq!(v1.target, oid(AVANTI));
    assert_eq!(
        v1.tag_object,
        Some(oid("aee2545a322748fed93caeb159cb0687647ff02c"))
    );
    // Tag del tag: git dà l'id di v1.0, il parser risale fino al commit.
    let doppio = find(&refs, "refs/tags/v1.0-doppio");
    assert_eq!(doppio.target, oid(AVANTI));
    assert_eq!(
        doppio.tag_object,
        Some(oid("f163c59c63da2efc141c560b416c3960be002938"))
    );
    // Leggero su un albero: il target è l'albero.
    assert_eq!(
        find(&refs, "refs/tags/albero"),
        &plain(
            "refs/tags/albero",
            RefKind::Tag,
            "7bee507cb7f5aa4dce34e6962366eeea27935c74"
        )
    );
}

#[test]
fn repo_vuoto() {
    assert_eq!(VUOTO, b"");
    assert_eq!(parse(VUOTO), []);
}

#[test]
fn upstream_con_a_capo_nel_nome() {
    assert_eq!(
        parse(UPSTREAM_A_CAPO),
        [branch(
            "refs/heads/main",
            "127556d36fcfa2e39fb5b05e1318fa8b8159eab3",
            Some(("refs/remotes/origin/riga\nnuova", None, None)),
        )]
    );
}

#[test]
fn tag_di_tag_senza_il_tag_interno_resta_al_tag_interno() {
    let a = "1111111111111111111111111111111111111111";
    let b = "2222222222222222222222222222222222222222";
    let input = format!("refs/tags/x\0{a}\0{b}\0\0\0\0\n");
    let refs = parse(input.as_bytes());
    assert_eq!(refs[0].target, oid(b));
    assert_eq!(refs[0].tag_object, Some(oid(a)));
}

#[test]
fn tipi_sconosciuti_ignorati() {
    let input = format!(
        "refs/notes/commits\0{BASE}\0\0\0\0\0\nrefs/stash/x\0{BASE}\0\0\0\0\0\nHEAD\0{BASE}\0\0\0\0\0\nrefs/heads/a\0{BASE}\0\0\0\0\0\n"
    );
    let refs = parse(input.as_bytes());
    assert_eq!(refs, [branch("refs/heads/a", BASE, None)]);
}

#[test]
fn ahead_behind() {
    for (track, expected) in [
        (&b""[..], Some((Some(0), Some(0)))),
        (b"gone", Some((None, None))),
        (b"ahead 3", Some((Some(3), Some(0)))),
        (b"behind 4", Some((Some(0), Some(4)))),
        (b"ahead 1, behind 2", Some((Some(1), Some(2)))),
        (b"ahead 4294967295", Some((Some(u32::MAX), Some(0)))),
        (b"ahead 4294967296", None),
        (b"ahead", None),
        (b"ahead -1", None),
        (b"[ahead 1]", None),
        (b"ahead 1,behind 2", None),
        (b"ahead 1, behind", None),
        (b"behind 2, ahead 1", None),
        (b"ahead 1, behind 2, x", None),
        (b"in pari", None),
    ] {
        assert_eq!(
            parse_track(track),
            expected,
            "{}",
            String::from_utf8_lossy(track)
        );
    }
}

#[test]
fn errori() {
    let ok = format!("refs/heads/a\0{BASE}\0\0\0\0\0\n");
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (
            ok.as_bytes()[..ok.len() - 1].to_vec(),
            "non termina con NUL e a capo",
        ),
        (b"refs/heads/a\n".to_vec(), "non termina con NUL e a capo"),
        (
            format!("refs/heads/a\0{BASE}\0\0\n").into_bytes(),
            "record n. 1: 3 campi invece di 6 (manca upstream",
        ),
        (
            format!("{ok}refs/heads/b\0\0\n").into_bytes(),
            "record n. 2: 2 campi invece di 6 (manca id puntato",
        ),
        (
            format!("refs/heads/a\0{BASE}\0\0\0\0\0refs/heads/b\0{BASE}\0\0\0\0\0\n").into_bytes(),
            "record n. 2: manca l'a capo",
        ),
        (b"refs/heads/a\0abc\0\0\0\0\0\n".to_vec(), "record n. 1: id"),
        (
            format!("refs/heads/a\0{BASE}\0xyz\0\0\0\0\n").into_bytes(),
            "record n. 1: id puntato dal tag",
        ),
        (
            format!("\0{BASE}\0\0\0\0\0\n").into_bytes(),
            "record n. 1: nome (%(refname)) vuoto",
        ),
        (
            format!("refs/heads/a\0{BASE}\0\0refs/remotes/o/a\0avanti 1\0\0\n").into_bytes(),
            "ahead/behind (%(upstream:track,nobracket)) non valido: `avanti 1`",
        ),
        (
            format!("refs/heads/a\0{BASE}\0\0\0ahead 1\0\0\n").into_bytes(),
            "è `ahead 1` ma upstream (%(upstream)) è vuoto",
        ),
    ];
    for (input, expected) in cases {
        let r = reason(parse_refs(&input).expect_err(&String::from_utf8_lossy(&input)));
        assert!(r.contains(expected), "`{r}` non contiene `{expected}`");
    }
}

/// Riscrive i ref nel formato di git: il contrario di [`parse_refs`].
fn render(refs: &[RefInfo]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in refs {
        let (object, peeled) = match r.tag_object {
            Some(tag) => (tag, r.target.to_string()),
            None => (r.target, String::new()),
        };
        let (upstream, track) = match &r.upstream {
            None => (String::new(), String::new()),
            Some(u) => {
                let track = match (u.ahead, u.behind) {
                    (None, _) | (_, None) => "gone".to_owned(),
                    (Some(0), Some(0)) => String::new(),
                    (Some(a), Some(0)) => format!("ahead {a}"),
                    (Some(0), Some(b)) => format!("behind {b}"),
                    (Some(a), Some(b)) => format!("ahead {a}, behind {b}"),
                };
                (u.name.clone(), track)
            }
        };
        out.extend_from_slice(
            format!("{}\0{object}\0{peeled}\0{upstream}\0{track}\0\0\n", r.name).as_bytes(),
        );
    }
    out
}

fn arb_oid() -> impl Strategy<Value = Oid> {
    any::<[u8; 20]>().prop_map(Oid::from_bytes)
}

fn arb_ref() -> impl Strategy<Value = RefInfo> {
    let local = (
        "[a-zA-Z0-9àè/._-]{1,20}",
        arb_oid(),
        prop::option::of((
            "refs/(heads|remotes/[a-z]{1,5})/[a-z/]{1,10}( \n[a-z]{0,3})?",
            prop::option::of((any::<u32>(), any::<u32>())),
        )),
    )
        .prop_map(|(name, target, upstream)| RefInfo {
            name: format!("refs/heads/{name}"),
            kind: RefKind::LocalBranch,
            target,
            tag_object: None,
            upstream: upstream.map(|(name, ab)| Upstream {
                name,
                ahead: ab.map(|(a, _)| a),
                behind: ab.map(|(_, b)| b),
            }),
        });
    let remote =
        ("[a-z]{1,5}/[a-zA-Z0-9/._-]{1,20}", arb_oid()).prop_map(|(name, target)| RefInfo {
            name: format!("refs/remotes/{name}"),
            kind: RefKind::RemoteBranch,
            target,
            tag_object: None,
            upstream: None,
        });
    let tag = (
        "[a-zA-Z0-9/._-]{1,20}",
        arb_oid(),
        prop::option::of(arb_oid()),
    )
        .prop_map(|(name, target, tag_object)| RefInfo {
            name: format!("refs/tags/{name}"),
            kind: RefKind::Tag,
            target,
            tag_object,
            upstream: None,
        });
    let stash = arb_oid().prop_map(|target| RefInfo {
        name: "refs/stash".into(),
        kind: RefKind::Stash,
        target,
        tag_object: None,
        upstream: None,
    });
    prop_oneof![local, remote, tag, stash]
}

proptest! {
    #[test]
    fn round_trip_del_formato(refs in prop::collection::vec(arb_ref(), 0..8)) {
        let parsed = parse_refs(&render(&refs)).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(parsed, refs);
    }

    #[test]
    fn round_trip_json(refs in prop::collection::vec(arb_ref(), 0..8)) {
        let json = serde_json::to_string(&refs).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let back: Vec<RefInfo> =
            serde_json::from_str(&json).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(back, refs);
    }

    #[test]
    fn nessun_panico_su_byte_qualsiasi(input in prop::collection::vec(any::<u8>(), 0..400)) {
        let _ = parse_refs(&input);
    }

    #[test]
    fn nessun_panico_su_output_reale_modificato(
        cut in 0..REFS.len(),
        pos in 0..REFS.len(),
        byte in any::<u8>(),
    ) {
        let _ = parse_refs(&REFS[..cut]);
        let mut changed = REFS.to_vec();
        changed[pos] = byte;
        let _ = parse_refs(&changed);
    }

    #[test]
    fn output_troncato_non_accettato_in_silenzio(cut in 1..REFS.len()) {
        // Un taglio può cadere solo su un confine di record, dopo `\0\n`: lì il risultato è un
        // prefisso della lista completa; altrove è un errore.
        let full = parse(REFS);
        match parse_refs(&REFS[..cut]) {
            Ok(refs) => {
                prop_assert!(REFS[..cut].ends_with(b"\0\n"));
                prop_assert!(full.starts_with(&refs));
            }
            Err(e) => { reason(e); }
        }
    }
}
