use super::*;
use crate::DomainError;

/// Real git output, generated in a temporary repo with two commits:
/// `git rev-parse HEAD HEAD~1 > rev-parse.txt` (git 2.43.0).
const REV_PARSE: &[u8] = include_bytes!("testdata/rev-parse.txt");

const HEX: &str = "8b07c4c7c77b9a070f20c81b2948722f1ccfac5d";

fn oid(hex: &str) -> Oid {
    Oid::from_hex(hex.as_bytes()).expect("valid test oid")
}

fn reason(err: DomainError) -> String {
    match err {
        DomainError::InvalidOid { reason, .. } => reason,
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn reads_oid_from_rev_parse_output() {
    let oids: Vec<Oid> = REV_PARSE
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(Oid::from_hex)
        .collect::<Result<_, _>>()
        .expect("lines valide");
    assert_eq!(oids.len(), 2);
    assert_eq!(oids[0].to_string(), HEX);
    assert_eq!(
        oids[1].to_string(),
        "3376592d1ce0ac9a87e616b17591bfc9b05c764d"
    );
    assert_ne!(oids[0], oids[1]);
}

#[test]
fn bytes_match_the_digits() {
    let id = oid(HEX);
    assert_eq!(id.as_bytes()[0], 0x8b);
    assert_eq!(id.as_bytes()[19], 0x5d);
    assert_eq!(Oid::from_bytes(*id.as_bytes()), id);
}

#[test]
fn accepts_uppercase_and_prints_lowercase() {
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
fn short_returns_seven_digits() {
    assert_eq!(oid(HEX).short(), "8b07c4c");
    assert_eq!(oid(&HEX.to_uppercase()).short(), "8b07c4c");
}

#[test]
fn rejects_empty_string() {
    let err = Oid::from_hex(b"").unwrap_err();
    assert_eq!(reason(err), "40 hex digits are needed, found 0");
}

#[test]
fn rejects_wrong_length() {
    assert!(Oid::from_hex(&HEX.as_bytes()[..39]).is_err());
    assert!(Oid::from_hex(format!("{HEX}0").as_bytes()).is_err());
    // short form: not a full oid
    let err = Oid::from_hex(b"8b07c4c").unwrap_err();
    assert_eq!(reason(err), "40 hex digits are needed, found 7");
    // trailing newline, as in git's raw output
    assert!(Oid::from_hex(format!("{HEX}\n").as_bytes()).is_err());
}

#[test]
fn rejects_non_hex_characters() {
    let mut bad = HEX.as_bytes().to_vec();
    bad[10] = b'g';
    let err = Oid::from_hex(&bad).unwrap_err();
    assert_eq!(reason(err), "non-hex character at position 11");

    let mut bad = HEX.as_bytes().to_vec();
    bad[0] = b' ';
    assert!(Oid::from_hex(&bad).is_err());

    // non-UTF-8 bytes: the error reports the input in readable form without panicking
    let mut bad = HEX.as_bytes().to_vec();
    bad[39] = 0xff;
    let err = Oid::from_hex(&bad).unwrap_err();
    assert!(err.to_string().contains("position 40"), "{err}");

    // 40 bytes but with a multibyte character: right length in bytes, wrong content
    let s = format!("{}é", &HEX[..38]);
    assert_eq!(s.len(), 40);
    assert!(s.parse::<Oid>().is_err());
}

#[test]
fn error_message_in_english() {
    let err = Oid::from_hex(b"xyz").unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid git identifier `xyz`: 40 hex digits are needed, found 3"
    );
}

#[test]
fn oid_in_json_as_string() {
    let id = oid(HEX);
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, format!("\"{HEX}\""));
    assert_eq!(serde_json::from_str::<Oid>(&json).unwrap(), id);
    let upper = format!("\"{}\"", HEX.to_uppercase());
    assert_eq!(serde_json::from_str::<Oid>(&upper).unwrap(), id);
}

#[test]
fn oid_json_invalid() {
    assert!(serde_json::from_str::<Oid>("\"abc\"").is_err());
    assert!(serde_json::from_str::<Oid>("42").is_err());
    assert!(serde_json::from_str::<Oid>("null").is_err());
    let err = serde_json::from_str::<Oid>("\"abc\"").unwrap_err();
    assert!(err.to_string().contains("invalid"), "{err}");
}

#[test]
fn oid_orderable_by_bytes() {
    let a = oid("0000000000000000000000000000000000000001");
    let b = oid("1000000000000000000000000000000000000000");
    assert!(a < b);
}

fn full_commit() -> Commit {
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
        summary: "Merge the feature branch".into(),
        body: "First line of the body.\n\nSecond line.".into(),
    }
}

#[test]
fn commit_round_trip_json() {
    let commit = full_commit();
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
fn root_commit_is_not_a_merge() {
    let mut commit = full_commit();
    commit.parents.clear();
    assert!(!commit.is_merge());
    let json = serde_json::to_string(&commit).unwrap();
    assert_eq!(serde_json::from_str::<Commit>(&json).unwrap(), commit);
}

#[test]
fn ref_and_head_round_trip_json() {
    let refs = vec![
        RefInfo {
            name: "refs/heads/main".into(),
            kind: RefKind::LocalBranch,
            target: oid(HEX),
            tag_object: None,
            upstream: Some(Upstream {
                name: "refs/remotes/origin/main".into(),
                ahead: Some(2),
                behind: Some(0),
            }),
        },
        RefInfo {
            name: "refs/heads/old".into(),
            kind: RefKind::LocalBranch,
            target: oid(HEX),
            tag_object: None,
            upstream: Some(Upstream {
                name: "refs/remotes/origin/old".into(),
                ahead: None,
                behind: None,
            }),
        },
        RefInfo {
            name: "refs/tags/v1.0".into(),
            kind: RefKind::Tag,
            target: oid(HEX),
            tag_object: Some(oid("0123456789abcdef0123456789abcdef01234567")),
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
fn short_ref_names() {
    let mk = |name: &str, kind| RefInfo {
        name: name.into(),
        kind,
        target: oid(HEX),
        tag_object: None,
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
    assert_eq!(mk("strange", RefKind::Tag).short_name(), "strange");
}

#[test]
fn head_target() {
    let id = oid(HEX);
    assert_eq!(Head::Detached(id).target(), Some(id));
    let empty = Head::Branch {
        name: "main".into(),
        target: None,
    };
    assert_eq!(empty.target(), None);
}
