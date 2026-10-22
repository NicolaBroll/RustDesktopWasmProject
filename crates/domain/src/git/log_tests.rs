use super::*;
use crate::DomainError;
use proptest::prelude::*;

/// Output reale di git 2.43.0, generato da `testdata/log.sh` (`sh log.sh testdata`) in un repo
/// temporaneo con `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, autore, committer e
/// date fissi. Il comando che lo produce è
///
/// ```sh
/// git log -z --date=raw --encoding=UTF-8 --no-show-signature --no-color \
///     --format='%H%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%s%x00%b'
/// ```
///
/// cioè gli argomenti di [`LOG_ARGS`] (`--format=X` equivale a `--format=tformat:X`).
/// Cinque commit, dal più recente: merge, messaggio Latin-1 dichiarato, autore Latin-1 non
/// dichiarato, corpo su più righe, radice.
const LOG: &[u8] = include_bytes!("testdata/log.bin");

/// Stesso comando di [`LOG`] sul range vuoto `HEAD..HEAD`: zero byte.
const LOG_VUOTO: &[u8] = include_bytes!("testdata/log-vuoto.bin");

fn oid(hex: &str) -> Oid {
    Oid::from_hex(hex.as_bytes()).expect("oid di test valido")
}

fn log() -> Vec<Commit> {
    parse_log(LOG).expect("log reale valido")
}

fn reason(err: DomainError) -> String {
    match err {
        DomainError::InvalidGitOutput { command, reason } => {
            assert_eq!(command, "log");
            reason
        }
        other => panic!("errore inatteso: {other:?}"),
    }
}

const ROOT: &str = "8893151e2dff32a80c69d111aa13df946e42a8a7";

fn root_oid() -> Oid {
    oid(ROOT)
}

#[test]
fn legge_tutti_i_commit_in_ordine() {
    let commits = log();
    let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(
        summaries,
        [
            "Merge laterale",
            "Caffè dichiarato Latin-1",
            "Autore non UTF-8",
            "Oggetto su due righe",
            "Commit radice",
        ]
    );
    // Ogni commit tranne la radice ha come genitore il commit successivo sul suo ramo.
    assert_eq!(commits[1].parents, [commits[2].id]);
    assert_eq!(commits[2].parents, [commits[3].id]);
    assert_eq!(commits[3].parents, [commits[4].id]);
}

#[test]
fn commit_radice_senza_genitori_e_corpo_vuoto() {
    let root = log().pop().expect("almeno un commit");
    assert_eq!(root.id, root_oid());
    assert!(root.parents.is_empty());
    assert!(!root.is_merge());
    assert_eq!(root.summary, "Commit radice");
    assert_eq!(root.body, "");
    assert_eq!(
        root.author,
        Signature {
            name: "Anna Autrice".into(),
            email: "anna@example.com".into(),
            time: 1_700_000_000,
            offset_min: 120,
        }
    );
    assert_eq!(root.committer.name, "Carlo Committer");
    assert_eq!(root.committer.email, "carlo@example.com");
}

#[test]
fn merge_con_due_genitori() {
    let commits = log();
    let merge = &commits[0];
    assert!(merge.is_merge());
    assert_eq!(merge.parents, [commits[3].id, commits[1].id]);
    assert_eq!(merge.body, "");
}

#[test]
fn corpo_su_piu_righe_e_oggetto_unito() {
    let c = &log()[3];
    // `%s` unisce le righe del primo paragrafo; `%b` è il resto, senza l'a capo finale.
    assert_eq!(c.summary, "Oggetto su due righe");
    assert_eq!(
        c.body,
        "Prima riga del corpo.\nSeconda riga del corpo.\n\nSecondo paragrafo."
    );
    assert_eq!(c.parents, [root_oid()]);
}

#[test]
fn fusi_orari_positivi_negativi_e_utc() {
    let commits = log();
    let offsets: Vec<(i32, i32)> = commits
        .iter()
        .map(|c| (c.author.offset_min, c.committer.offset_min))
        .collect();
    assert_eq!(
        offsets,
        [
            (-7 * 60, 12 * 60 + 45),
            (60, 60),
            (-(3 * 60 + 30), 0),
            (5 * 60 + 30, -8 * 60),
            (120, 120),
        ]
    );
    // I secondi sono UTC, indipendenti dal fuso.
    assert_eq!(commits[0].author.time, 1_700_000_400);
    assert_eq!(commits[3].committer.time, 1_700_000_100);
}

#[test]
fn autore_non_utf8_diventa_testo_con_sostituzione() {
    let c = &log()[2];
    // Il nome nel commit è `Jos\xe9 Latin1` senza intestazione `encoding`: git lo passa così
    // com'è e il parser lo rende leggibile con `from_utf8_lossy`.
    assert_eq!(c.author.name, "Jos\u{fffd} Latin1");
    assert_eq!(c.author.email, "jose@example.com");
    assert_eq!(c.summary, "Autore non UTF-8");
}

#[test]
fn codifica_dichiarata_convertita_da_git() {
    let c = &log()[1];
    // Commit con `encoding ISO-8859-1`: `--encoding=UTF-8` lo fa convertire a git.
    assert_eq!(c.author.name, "Renée Encoding");
    assert_eq!(c.summary, "Caffè dichiarato Latin-1");
    assert_eq!(c.body, "Corpo in Latin-1: perché.");
}

#[test]
fn output_vuoto_nessun_commit() {
    assert!(LOG_VUOTO.is_empty());
    assert_eq!(parse_log(LOG_VUOTO), Ok(Vec::new()));
}

#[test]
fn argomenti_contengono_il_formato() {
    assert!(LOG_ARGS.contains(&"-z"));
    assert!(LOG_ARGS.contains(&"--date=raw"));
    let format = format!("--format=tformat:{LOG_FORMAT}");
    assert!(LOG_ARGS.contains(&format.as_str()));
    assert_eq!(LOG_FORMAT.matches("%x00").count(), LOG_FIELDS - 1);
}

#[test]
fn json_del_commit() {
    let root = log().pop().expect("almeno un commit");
    let json = serde_json::to_value(&root).expect("serializzabile");
    assert_eq!(json["id"], root_oid().to_string());
    assert_eq!(json["author"]["offset_min"], 120);
    let back: Commit = serde_json::from_value(json).expect("deserializzabile");
    assert_eq!(back, root);
}

// --- Input malformati -------------------------------------------------------------------

/// Il primo commit del log reale, come record a sé (dieci campi terminati da NUL).
fn first_record() -> Vec<u8> {
    let mut nul = 0;
    let end = LOG
        .iter()
        .position(|&b| {
            nul += usize::from(b == 0);
            nul == LOG_FIELDS
        })
        .expect("almeno un record");
    LOG[..=end].to_vec()
}

fn with_field(idx: usize, value: &[u8]) -> Vec<u8> {
    let rec = first_record();
    let mut fields: Vec<&[u8]> = rec[..rec.len() - 1].split(|&b| b == 0).collect();
    fields[idx] = value;
    let mut out = fields.join(&0u8);
    out.push(0);
    out
}

#[test]
fn record_isolato_valido() {
    let commits = parse_log(&first_record()).expect("valido");
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0], log()[0]);
}

#[test]
fn errore_se_manca_il_nul_finale() {
    // Output troncato a metà di un campo (qui l'id di un sesto commit).
    let mut input = LOG.to_vec();
    input.extend_from_slice(b"8893151e");
    assert!(reason(parse_log(&input).unwrap_err()).contains("non termina con NUL"));
    // Senza l'ultimo NUL ma con il corpo vuoto l'input finisce ancora con NUL: risulta un
    // campo in meno.
    let mut input = first_record();
    input.pop();
    assert!(reason(parse_log(&input).unwrap_err()).contains("ha 9 campi"));
}

#[test]
fn errore_se_mancano_campi() {
    let rec = first_record();
    let mut input = LOG.to_vec();
    // Secondo record con solo tre campi.
    input.extend_from_slice(b"a\0b\0c\0");
    let r = reason(parse_log(&input).unwrap_err());
    assert!(r.contains("commit n. 6 ha 3 campi invece di 10"), "{r}");

    // Un campo in meno: il record non si chiude.
    let cut = rec.iter().rposition(|&b| b == 0).expect("nul");
    let short = &rec[..cut];
    let short = &short[..=short.iter().rposition(|&b| b == 0).expect("nul")];
    let r = reason(parse_log(short).unwrap_err());
    assert!(r.contains("commit n. 1 ha 9 campi"), "{r}");
}

#[test]
fn errore_se_l_id_non_e_valido() {
    let r = reason(parse_log(&with_field(0, b"1234")).unwrap_err());
    assert!(r.contains("commit n. 1, campo id (%H)"), "{r}");
    assert!(r.contains("`1234` non è un id valido"), "{r}");
}

#[test]
fn errore_se_un_genitore_non_e_valido() {
    let mut parents = ROOT.as_bytes().to_vec();
    parents.extend_from_slice(b" zz");
    let r = reason(parse_log(&with_field(1, &parents)).unwrap_err());
    assert!(r.contains("campo genitori (%P)"), "{r}");
}

#[test]
fn errore_se_la_data_non_e_valida() {
    let casi: [(&[u8], &str); 6] = [
        (b"1700000000", "manca il fuso orario"),
        (b"ieri +0200", "secondi"),
        (b"-5 +0200", "secondi"),
        (b"1700000000 0200", "deve iniziare con `+` o `-`"),
        (b"1700000000 +200", "quattro cifre"),
        (b"1700000000 +02x0", "minuti del fuso"),
    ];
    for (data, attesa) in casi {
        let r = reason(parse_log(&with_field(4, data)).unwrap_err());
        assert!(r.contains("campo data dell'autore (%ad)"), "{r}");
        assert!(r.contains(attesa), "{data:?}: {r}");
    }
    let r = reason(parse_log(&with_field(7, b"")).unwrap_err());
    assert!(r.contains("campo data del committer (%cd)"), "{r}");
}

// --- Proprietà --------------------------------------------------------------------------

/// Scrive i commit come farebbe git con [`LOG_ARGS`]; `%b` riceve un a capo finale se non vuoto.
fn render(commits: &[Commit]) -> Vec<u8> {
    let mut out = Vec::new();
    for c in commits {
        let parents: Vec<String> = c.parents.iter().map(Oid::to_string).collect();
        let date = |s: &Signature| {
            let sign = if s.offset_min < 0 { '-' } else { '+' };
            let m = s.offset_min.abs();
            format!("{} {sign}{:02}{:02}", s.time, m / 60, m % 60)
        };
        let body = if c.body.is_empty() {
            String::new()
        } else {
            format!("{}\n", c.body)
        };
        let fields = [
            c.id.to_string(),
            parents.join(" "),
            c.author.name.clone(),
            c.author.email.clone(),
            date(&c.author),
            c.committer.name.clone(),
            c.committer.email.clone(),
            date(&c.committer),
            c.summary.clone(),
            body,
        ];
        for f in fields {
            out.extend_from_slice(f.as_bytes());
            out.push(0);
        }
    }
    out
}

fn arb_oid() -> impl Strategy<Value = Oid> {
    any::<[u8; 20]>().prop_map(Oid::from_bytes)
}

fn arb_signature() -> impl Strategy<Value = Signature> {
    (
        "[^\0]{0,12}",
        "[^\0]{0,12}",
        0i64..=4_000_000_000,
        -(99 * 60 + 59)..=(99 * 60 + 59),
    )
        .prop_map(|(name, email, time, offset_min)| Signature {
            name,
            email,
            time,
            offset_min,
        })
}

fn arb_commit() -> impl Strategy<Value = Commit> {
    (
        arb_oid(),
        prop::collection::vec(arb_oid(), 0..4),
        arb_signature(),
        arb_signature(),
        "[^\0\n]{0,20}",
        "([^\0]{0,30}[^\0\n])?",
    )
        .prop_map(|(id, parents, author, committer, summary, body)| Commit {
            id,
            parents,
            author,
            committer,
            summary,
            body,
        })
}

proptest! {
    #[test]
    fn round_trip_del_formato(commits in prop::collection::vec(arb_commit(), 0..6)) {
        let parsed = parse_log(&render(&commits)).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(parsed, commits);
    }

    #[test]
    fn nessun_panico_su_byte_qualsiasi(input in prop::collection::vec(any::<u8>(), 0..400)) {
        let _ = parse_log(&input);
    }

    #[test]
    fn nessun_panico_su_record_alterati(idx in 0..LOG_FIELDS, value in "[ -~]{0,50}") {
        let _ = parse_log(&with_field(idx, value.as_bytes()));
    }
}
