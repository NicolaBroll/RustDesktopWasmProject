use super::*;
use crate::DomainError;

/// Output reale di git, generato in un repo temporaneo con due commit:
/// `git rev-parse HEAD HEAD~1 > rev-parse.txt` (git 2.43.0).
const REV_PARSE: &[u8] = include_bytes!("testdata/rev-parse.txt");

const HEX: &str = "8b07c4c7c77b9a070f20c81b2948722f1ccfac5d";

fn oid(hex: &str) -> Oid {
    Oid::from_hex(hex.as_bytes()).expect("oid di test valido")
}

fn reason(err: DomainError) -> String {
    match err {
        DomainError::InvalidOid { reason, .. } => reason,
        other => panic!("errore inatteso: {other:?}"),
    }
}

#[test]
fn legge_oid_dall_output_di_rev_parse() {
    let oids: Vec<Oid> = REV_PARSE
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(Oid::from_hex)
        .collect::<Result<_, _>>()
        .expect("righe valide");
    assert_eq!(oids.len(), 2);
    assert_eq!(oids[0].to_string(), HEX);
    assert_eq!(
        oids[1].to_string(),
        "3376592d1ce0ac9a87e616b17591bfc9b05c764d"
    );
    assert_ne!(oids[0], oids[1]);
}

#[test]
fn byte_corrispondono_alle_cifre() {
    let id = oid(HEX);
    assert_eq!(id.as_bytes()[0], 0x8b);
    assert_eq!(id.as_bytes()[19], 0x5d);
    assert_eq!(Oid::from_bytes(*id.as_bytes()), id);
}

#[test]
fn accetta_maiuscole_e_stampa_minuscole() {
    let upper = oid(&HEX.to_uppercase());
    assert_eq!(upper, oid(HEX));
    assert_eq!(upper.to_string(), HEX);
    let mixed: Oid = "8B07c4C7c77b9a070f20c81b2948722f1ccfac5D".parse().unwrap();
    assert_eq!(mixed, upper);
}

#[test]
fn round_trip_display_parse() {
    for hex in [
        HEX,
        "0000000000000000000000000000000000000000",
        "ffffffffffffffffffffffffffffffffffffffff",
        "0123456789abcdef0123456789abcdef01234567",
    ] {
        let id = oid(hex);
        assert_eq!(id.to_string(), hex);
        assert_eq!(id.to_string().parse::<Oid>().unwrap(), id);
    }
}

#[test]
fn short_restituisce_sette_cifre() {
    assert_eq!(oid(HEX).short(), "8b07c4c");
    assert_eq!(oid(&HEX.to_uppercase()).short(), "8b07c4c");
}

#[test]
fn rifiuta_stringa_vuota() {
    let err = Oid::from_hex(b"").unwrap_err();
    assert_eq!(reason(err), "servono 40 cifre esadecimali, trovate 0");
}

#[test]
fn rifiuta_lunghezza_sbagliata() {
    assert!(Oid::from_hex(&HEX.as_bytes()[..39]).is_err());
    assert!(Oid::from_hex(format!("{HEX}0").as_bytes()).is_err());
    // forma abbreviata: non è un oid completo
    let err = Oid::from_hex(b"8b07c4c").unwrap_err();
    assert_eq!(reason(err), "servono 40 cifre esadecimali, trovate 7");
    // a capo finale, come nell'output grezzo di git
    assert!(Oid::from_hex(format!("{HEX}\n").as_bytes()).is_err());
}

#[test]
fn rifiuta_caratteri_non_esadecimali() {
    let mut bad = HEX.as_bytes().to_vec();
    bad[10] = b'g';
    let err = Oid::from_hex(&bad).unwrap_err();
    assert_eq!(reason(err), "carattere non esadecimale in posizione 11");

    let mut bad = HEX.as_bytes().to_vec();
    bad[0] = b' ';
    assert!(Oid::from_hex(&bad).is_err());

    // byte non UTF-8: l'errore riporta l'input in forma leggibile senza panico
    let mut bad = HEX.as_bytes().to_vec();
    bad[39] = 0xff;
    let err = Oid::from_hex(&bad).unwrap_err();
    assert!(err.to_string().contains("posizione 40"), "{err}");

    // 40 byte ma con un carattere multibyte: lunghezza giusta in byte, contenuto no
    let s = format!("{}è", &HEX[..38]);
    assert_eq!(s.len(), 40);
    assert!(s.parse::<Oid>().is_err());
}

#[test]
fn messaggio_di_errore_in_italiano() {
    let err = Oid::from_hex(b"xyz").unwrap_err();
    assert_eq!(
        err.to_string(),
        "identificatore git `xyz` non valido: servono 40 cifre esadecimali, trovate 3"
    );
}

#[test]
fn oid_in_json_come_stringa() {
    let id = oid(HEX);
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, format!("\"{HEX}\""));
    assert_eq!(serde_json::from_str::<Oid>(&json).unwrap(), id);
    let upper = format!("\"{}\"", HEX.to_uppercase());
    assert_eq!(serde_json::from_str::<Oid>(&upper).unwrap(), id);
}

#[test]
fn oid_json_non_valido() {
    assert!(serde_json::from_str::<Oid>("\"abc\"").is_err());
    assert!(serde_json::from_str::<Oid>("42").is_err());
    assert!(serde_json::from_str::<Oid>("null").is_err());
    let err = serde_json::from_str::<Oid>("\"abc\"").unwrap_err();
    assert!(err.to_string().contains("non valido"), "{err}");
}

#[test]
fn oid_ordinabile_per_byte() {
    let a = oid("0000000000000000000000000000000000000001");
    let b = oid("1000000000000000000000000000000000000000");
    assert!(a < b);
}

fn commit_completo() -> Commit {
    let author = Signature {
        name: "Anna Rossi".into(),
        email: "anna@example.com".into(),
        time: 1_704_067_200,
        offset_min: 60,
    };
    Commit {
        id: oid(HEX),
        parents: vec![
            oid("3376592d1ce0ac9a87e616b17591bfc9b05c764d"),
            oid("0123456789abcdef0123456789abcdef01234567"),
        ],
        committer: Signature {
            name: "Bruno".into(),
            email: "bruno@example.com".into(),
            time: 1_704_070_800,
            offset_min: -300,
        },
        author,
        summary: "Unisci il branch funzione".into(),
        body: "Prima riga del corpo.\n\nSeconda riga.".into(),
    }
}

#[test]
fn commit_round_trip_json() {
    let commit = commit_completo();
    assert!(commit.is_merge());
    let json = serde_json::to_value(&commit).unwrap();
    assert_eq!(json["id"], HEX);
    assert_eq!(
        json["parents"][0],
        "3376592d1ce0ac9a87e616b17591bfc9b05c764d"
    );
    assert_eq!(json["author"]["offset_min"], 60);
    let back: Commit = serde_json::from_value(json).unwrap();
    assert_eq!(back, commit);
}

#[test]
fn commit_radice_non_e_merge() {
    let mut commit = commit_completo();
    commit.parents.clear();
    assert!(!commit.is_merge());
    let json = serde_json::to_string(&commit).unwrap();
    assert_eq!(serde_json::from_str::<Commit>(&json).unwrap(), commit);
}

#[test]
fn ref_e_head_round_trip_json() {
    let refs = vec![
        RefInfo {
            name: "refs/heads/main".into(),
            kind: RefKind::LocalBranch,
            target: oid(HEX),
            upstream: Some(Upstream {
                name: "refs/remotes/origin/main".into(),
                ahead: Some(2),
                behind: Some(0),
            }),
        },
        RefInfo {
            name: "refs/heads/vecchio".into(),
            kind: RefKind::LocalBranch,
            target: oid(HEX),
            upstream: Some(Upstream {
                name: "refs/remotes/origin/vecchio".into(),
                ahead: None,
                behind: None,
            }),
        },
        RefInfo {
            name: "refs/tags/v1.0".into(),
            kind: RefKind::Tag,
            target: oid(HEX),
            upstream: None,
        },
    ];
    let json = serde_json::to_string(&refs).unwrap();
    assert_eq!(serde_json::from_str::<Vec<RefInfo>>(&json).unwrap(), refs);

    for head in [
        Head::Branch {
            name: "main".into(),
            target: Some(oid(HEX)),
        },
        Head::Branch {
            name: "main".into(),
            target: None,
        },
        Head::Detached(oid(HEX)),
    ] {
        let json = serde_json::to_string(&head).unwrap();
        assert_eq!(serde_json::from_str::<Head>(&json).unwrap(), head);
    }
}

#[test]
fn nomi_brevi_dei_ref() {
    let mk = |name: &str, kind| RefInfo {
        name: name.into(),
        kind,
        target: oid(HEX),
        upstream: None,
    };
    assert_eq!(
        mk("refs/heads/feature/x", RefKind::LocalBranch).short_name(),
        "feature/x"
    );
    assert_eq!(
        mk("refs/remotes/origin/main", RefKind::RemoteBranch).short_name(),
        "origin/main"
    );
    assert_eq!(mk("refs/tags/v1.0", RefKind::Tag).short_name(), "v1.0");
    assert_eq!(mk("refs/stash", RefKind::Stash).short_name(), "stash");
    assert_eq!(mk("strano", RefKind::Tag).short_name(), "strano");
}

#[test]
fn target_di_head() {
    let id = oid(HEX);
    assert_eq!(Head::Detached(id).target(), Some(id));
    let vuoto = Head::Branch {
        name: "main".into(),
        target: None,
    };
    assert_eq!(vuoto.target(), None);
}
