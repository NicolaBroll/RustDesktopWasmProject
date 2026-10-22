use super::*;
use crate::DomainError;
use proptest::prelude::*;

// Input reali di git 2.43.0, generati da `testdata/patch.sh` (`sh patch.sh testdata`) in un repo
// temporaneo con `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1` e date fisse, con
// `git -c core.quotePath=false diff` e gli argomenti di [`DIFF_ARGS`].

/// `d` (worktree → indice), input dello stage: due hunk vicini con più modifiche
/// (`vicini.txt`), fine file senza a capo in ogni combinazione, CRLF, contenuto non UTF-8, nome
/// non UTF-8, file eliminato dal worktree, file `git add -N` (nuovo), modo cambiato con righe.
const WORKTREE: &[u8] = include_bytes!("testdata/patch-worktree.bin");
/// `d --cached` (HEAD → indice), input dell'unstage: due hunk con modifiche vicine, a capo
/// finale aggiunto, entrambi senza a capo, CRLF, file nuovo, file eliminato, rinomina con
/// modifiche verso un nome con spazio, nome con tab (tra virgolette).
const INDICE: &[u8] = include_bytes!("testdata/patch-indice.bin");
/// Lo script che genera i file: deve usare gli stessi argomenti di [`DIFF_ARGS`].
const SCRIPT: &str = include_str!("testdata/patch.sh");

// Per i casi da rifiutare servono i file dei test del parser (`testdata/diff.sh`).
const FILE: &[u8] = include_bytes!("testdata/diff-file.bin");
const SOTTOMODULO: &[u8] = include_bytes!("testdata/diff-sottomodulo.bin");
const CONFLITTO: &[u8] = include_bytes!("testdata/diff-conflitto.bin");

use LineRef::{New, Old};
use PatchDirection::{Stage, Unstage};

fn file(input: &[u8], path: &[u8]) -> FileDiff {
    let diff = parse_diff(input).expect("output reale valido");
    let mut found = diff
        .files
        .into_iter()
        .filter(|f| f.new_path.as_bytes() == path);
    let f = found.next().expect("file presente");
    assert!(found.next().is_none(), "un solo file con questo percorso");
    f
}

fn patch(f: &FileDiff, lines: &[LineRef], direction: PatchDirection) -> FileDiff {
    let p = build_patch(f, lines, direction)
        .expect("patch costruita")
        .expect("patch non vuota");
    // La patch è un diff valido: si rilegge identica.
    let bytes = p.to_patch();
    assert_eq!(
        parse_diff(&bytes).expect("patch leggibile").files,
        std::slice::from_ref(&p)
    );
    p
}

/// La patch come testo per gli snapshot, senza perdere byte: un tab vero è `→`, un `\r` è `␍`,
/// un byte non UTF-8 o di controllo è `⟨xx⟩` in esadecimale.
fn render(p: &FileDiff) -> String {
    let mut out = String::new();
    for chunk in p.to_patch().utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '\t' => out.push('→'),
                '\r' => out.push('␍'),
                '\n' => out.push('\n'),
                c if c.is_control() => out.push_str(&format!("⟨{:02x}⟩", c as u32)),
                c => out.push(c),
            }
        }
        for b in chunk.invalid() {
            out.push_str(&format!("⟨{b:02x}⟩"));
        }
    }
    out
}

fn snapshot(name: &str, input: &[u8], path: &[u8], lines: &[LineRef], dir: PatchDirection) {
    let f = file(input, path);
    insta::assert_snapshot!(name, render(&patch(&f, lines, dir)));
}

#[test]
fn lo_script_usa_gli_argomenti_del_parser() {
    let args = DIFF_ARGS.join(" ");
    assert!(SCRIPT.contains(&format!("git -c core.quotePath=false diff {args} \"$@\"")));
}

// --- Stage (diff worktree) -------------------------------------------------------------

#[test]
fn stage_hunk_vicini() {
    // Solo la sostituzione della riga 1: la vecchia riga 2 resta come contesto, la riga 2 del
    // worktree (aggiunta non selezionata) sparisce, come le modifiche più avanti.
    snapshot(
        "stage_vicini_parziale",
        WORKTREE,
        b"vicini.txt",
        &[Old(1), New(1)],
        Stage,
    );
    // Una riga per gruppo, in gruppi diversi dello stesso hunk.
    snapshot(
        "stage_vicini_sparse",
        WORKTREE,
        b"vicini.txt",
        &[Old(4), New(9), Old(16)],
        Stage,
    );
}

#[test]
fn stage_selezione_totale_uguale_al_diff() {
    let diff = parse_diff(WORKTREE).unwrap();
    for f in diff.files.iter().filter(|f| f.change == Change::Modified) {
        let p = patch(f, &f.line_refs(), Stage);
        assert_eq!(p.hunks, f.hunks, "{}", f.path());
    }
}

#[test]
fn stage_fine_file_senza_a_capo() {
    // `-b⏎\ No newline`, `+b`, `+c\ No newline`: solo `+c` dà la stessa patch del totale,
    // perché `b` prende l'a capo.
    snapshot("stage_coda_solo_c", WORKTREE, b"coda.txt", &[New(3)], Stage);
    // Solo `+b` (con a capo): la vecchia `b` resta, prende l'a capo, e si aggiunge `b`.
    snapshot("stage_coda_solo_b", WORKTREE, b"coda.txt", &[New(2)], Stage);
    // Solo `-b`: il file finisce con `a` e il suo a capo.
    snapshot(
        "stage_coda_solo_tolta",
        WORKTREE,
        b"coda.txt",
        &[Old(2)],
        Stage,
    );
    // Solo la riga senza a capo aggiunta: la vecchia resta come contesto con il suo a capo.
    snapshot(
        "stage_toglie_a_capo_solo_aggiunta",
        WORKTREE,
        b"toglie-a-capo.txt",
        &[New(2)],
        Stage,
    );
    // Riga aggiunta prima dell'ultima riga senza a capo (contesto): il marker resta lì.
    snapshot(
        "stage_entrambi_senza",
        WORKTREE,
        b"entrambi-senza.txt",
        &[New(1)],
        Stage,
    );
}

#[test]
fn stage_byte_esatti() {
    snapshot("stage_crlf", WORKTREE, b"crlf.txt", &[New(4)], Stage);
    snapshot("stage_latin1", WORKTREE, b"latin1.txt", &[New(3)], Stage);
    snapshot(
        "stage_nome_non_utf8",
        WORKTREE,
        b"nome\xff.txt",
        &[New(4)],
        Stage,
    );

    let p = patch(&file(WORKTREE, b"crlf.txt"), &[New(4)], Stage).to_patch();
    assert!(p.windows(10).any(|w| w == b"+quattro\r\n"));
    let p = patch(&file(WORKTREE, b"latin1.txt"), &[New(3)], Stage).to_patch();
    assert!(p.windows(7).any(|w| w == b" caff\xe8\n"));
    let p = patch(&file(WORKTREE, b"nome\xff.txt"), &[New(4)], Stage).to_patch();
    assert!(p.starts_with(b"diff --git a/nome\xff.txt b/nome\xff.txt\n"));
}

#[test]
fn stage_file_nuovo_e_eliminato() {
    // File `git add -N`: si crea con le sole righe scelte.
    snapshot(
        "stage_nuovo_parziale",
        WORKTREE,
        b"intento.txt",
        &[New(2)],
        Stage,
    );
    // File eliminato dal worktree: parziale = modifica, totale = eliminazione.
    snapshot(
        "stage_eliminato_parziale",
        WORKTREE,
        b"nuovo nome.txt",
        &[Old(7)],
        Stage,
    );
    let f = file(WORKTREE, b"nuovo nome.txt");
    let p = patch(&f, &f.line_refs(), Stage);
    assert_eq!(p.change, Change::Deleted);
    assert_eq!(p.old_mode, Some(0o100644));
    snapshot(
        "stage_eliminato_totale",
        WORKTREE,
        b"nuovo nome.txt",
        &f.line_refs(),
        Stage,
    );
}

#[test]
fn stage_senza_cambio_di_modo() {
    let f = file(WORKTREE, b"script.sh");
    assert_eq!((f.old_mode, f.new_mode), (Some(0o100644), Some(0o100755)));
    let p = patch(&f, &[New(2)], Stage);
    assert_eq!((p.old_mode, p.new_mode), (None, None));
    snapshot("stage_modo", WORKTREE, b"script.sh", &[New(2)], Stage);
}

// --- Unstage (diff dell'indice) ----------------------------------------------------------

#[test]
fn unstage_hunk_vicini() {
    snapshot(
        "unstage_vicini_riga_2",
        INDICE,
        b"vicini.txt",
        &[Old(2), New(2)],
        Unstage,
    );
    // Solo la riga aggiunta in stage: esce dall'indice, la vecchia non torna.
    snapshot(
        "unstage_vicini_solo_aggiunta",
        INDICE,
        b"vicini.txt",
        &[New(16)],
        Unstage,
    );
}

#[test]
fn unstage_fine_file_senza_a_capo() {
    // `-senza a capo⏎\ No newline`, `+senza a capo`, `+più`: solo `più` esce dall'indice.
    snapshot(
        "unstage_aggiunge_a_capo_solo_piu",
        INDICE,
        b"aggiunge-a-capo.txt",
        &[New(3)],
        Unstage,
    );
    // Solo la vecchia riga senza a capo torna: non è l'ultima, prende l'a capo.
    snapshot(
        "unstage_aggiunge_a_capo_solo_tolta",
        INDICE,
        b"aggiunge-a-capo.txt",
        &[Old(2)],
        Unstage,
    );
    snapshot(
        "unstage_entrambi_senza",
        INDICE,
        b"entrambi-senza.txt",
        &[Old(1), New(1)],
        Unstage,
    );
}

#[test]
fn unstage_file_nuovo_eliminato_rinominato() {
    // File nuovo in stage: parziale = modifica, totale = eliminazione dall'indice.
    snapshot(
        "unstage_nuovo_parziale",
        INDICE,
        b"nuovo.txt",
        &[New(1)],
        Unstage,
    );
    let f = file(INDICE, b"nuovo.txt");
    let p = patch(&f, &f.line_refs(), Unstage);
    assert_eq!(p.change, Change::Deleted);
    snapshot(
        "unstage_nuovo_totale",
        INDICE,
        b"nuovo.txt",
        &f.line_refs(),
        Unstage,
    );
    // File eliminato in stage: torna nell'indice con le sole righe scelte.
    snapshot(
        "unstage_eliminato_parziale",
        INDICE,
        b"eliminato.txt",
        &[Old(2)],
        Unstage,
    );
    // Rinomina in stage: la patch modifica il percorso nuovo, senza intestazioni di rinomina.
    let f = file(INDICE, b"nuovo nome.txt");
    assert_eq!(f.change, Change::Renamed);
    let p = patch(&f, &[Old(7), New(7)], Unstage);
    assert_eq!(p.change, Change::Modified);
    assert_eq!(p.old_path, RepoPath::from("nuovo nome.txt"));
    snapshot(
        "unstage_rinomina",
        INDICE,
        b"nuovo nome.txt",
        &[Old(7), New(7)],
        Unstage,
    );
}

#[test]
fn unstage_byte_esatti() {
    snapshot(
        "unstage_crlf",
        INDICE,
        b"crlf.txt",
        &[Old(3), New(3)],
        Unstage,
    );
    snapshot(
        "unstage_nome_con_tab",
        INDICE,
        b"con\ttab.txt",
        &[New(4)],
        Unstage,
    );
    let p = patch(&file(INDICE, b"con\ttab.txt"), &[New(4)], Unstage).to_patch();
    assert!(p.starts_with(b"diff --git \"a/con\\ttab.txt\" \"b/con\\ttab.txt\"\n"));
}

// --- Selezione ---------------------------------------------------------------------------

#[test]
fn selezione_vuota_nessuna_patch() {
    let diff = parse_diff(WORKTREE).unwrap();
    for f in &diff.files {
        assert_eq!(build_patch(f, &[], Stage), Ok(None), "{}", f.path());
    }
    let f = file(INDICE, b"vicini.txt");
    assert_eq!(build_patch(&f, &[], Unstage), Ok(None));
}

#[test]
fn selezione_ripetuta_vale_una_volta() {
    let f = file(WORKTREE, b"vicini.txt");
    assert_eq!(
        build_patch(&f, &[New(9), New(9), Old(9)], Stage),
        build_patch(&f, &[Old(9), New(9)], Stage)
    );
}

fn selection_reason(f: &FileDiff, lines: &[LineRef], dir: PatchDirection) -> String {
    match build_patch(f, lines, dir) {
        Err(DomainError::InvalidLineSelection { path, reason }) => {
            assert_eq!(path, f.path().to_string_lossy());
            reason
        }
        other => panic!("errore di selezione atteso, ottenuto {other:?}"),
    }
}

#[test]
fn selezione_di_righe_inesistenti_rifiutata() {
    let f = file(WORKTREE, b"vicini.txt");
    // Riga di contesto (la 3 è uguale nei due lati), numero fuori dal diff, lato sbagliato.
    let r = selection_reason(&f, &[Old(1), Old(3)], Stage);
    assert!(r.contains("tolta n. 3"), "{r}");
    let r = selection_reason(&f, &[New(99), Old(99)], Stage);
    assert!(r.contains("tolta n. 99, aggiunta n. 99"), "{r}");
    // `riga 9 nel worktree` è la nuova riga 9: non c'è una riga tolta n. 10.
    let r = selection_reason(&f, &[Old(10)], Stage);
    assert!(r.contains("tolta n. 10"), "{r}");
    let msg = DomainError::InvalidLineSelection {
        path: "vicini.txt".into(),
        reason: r,
    }
    .to_string();
    assert!(msg.starts_with("selezione di righe non valida per `vicini.txt`:"));
    // Un cambio di solo modo non ha righe.
    let f = file(FILE, b"eseguibile.sh");
    selection_reason(&f, &[New(1)], Stage);
}

fn unsupported(f: &FileDiff, dir: PatchDirection) -> String {
    match build_patch(f, &f.line_refs(), dir) {
        Err(DomainError::UnsupportedPatch { reason, .. }) => reason,
        other => panic!("rifiuto atteso per {}, ottenuto {other:?}", f.path()),
    }
}

#[test]
fn file_non_adatti_rifiutati() {
    for path in [
        &b"binario-nuovo.bin"[..],
        b"binario-eliminato.bin",
        b"immagine.gif",
    ] {
        let f = file(FILE, path);
        assert!(unsupported(&f, Stage).contains("binario"));
        assert!(unsupported(&f, Unstage).contains("binario"));
    }
    let f = file(SOTTOMODULO, b"sub");
    assert!(unsupported(&f, Stage).contains("sottomodulo"));
    let diff = parse_diff(FILE).unwrap();
    let link = diff
        .files
        .iter()
        .find(|f| f.new_mode == Some(0o120000))
        .unwrap();
    assert!(unsupported(link, Unstage).contains("link simbolico"));
    let f = file(CONFLITTO, b"conflitto.txt");
    assert!(unsupported(&f, Stage).contains("conflitto"));
    // Anche con la selezione vuota: il file non si presta.
    assert!(matches!(
        build_patch(&f, &[], Stage),
        Err(DomainError::UnsupportedPatch { .. })
    ));
    let f = file(FILE, b"rinominata-modificata.txt");
    assert!(unsupported(&f, Stage).contains("rinomina"));
    assert!(build_patch(&f, &f.line_refs(), Unstage).is_ok());
    let mut copy = f.clone();
    copy.change = Change::Copied;
    assert!(unsupported(&copy, Unstage).contains("copia"));
}

#[test]
fn riferimenti_di_riga_in_json() {
    let refs = vec![Old(3), New(5)];
    let json = serde_json::to_string(&refs).unwrap();
    assert_eq!(json, r#"[{"old":3},{"new":5}]"#);
    assert_eq!(serde_json::from_str::<Vec<LineRef>>(&json).unwrap(), refs);
    assert_eq!(serde_json::to_string(&Unstage).unwrap(), r#""unstage""#);
    let f = file(WORKTREE, b"vicini.txt");
    assert_eq!(
        f.hunks[0].line_refs(),
        [
            Old(1),
            Old(2),
            New(1),
            New(2),
            Old(4),
            New(4),
            Old(9),
            New(9),
            Old(15),
            Old(16),
            New(15),
            Old(18),
            New(17)
        ]
    );
}

// --- Test di proprietà -------------------------------------------------------------------
//
// Un diff casuale nasce da un copione di righe (tenute, tolte, aggiunte), raggruppate in hunk
// con 3 righe di contesto come fa git. Un piccolo applicatore puro applica una patch a un
// contenuto e verifica intestazioni, conteggi e contesto: il risultato si confronta con il
// contenuto atteso calcolato dal copione.

#[derive(Debug, Clone)]
struct Script {
    lines: Vec<Line>,
}

const CONTEXT: usize = 3;

fn script_strategy() -> impl Strategy<Value = Script> {
    let piece = prop_oneof![
        Just(&b"a"[..]),
        Just(&b"b"[..]),
        Just(&b"c\r"[..]),
        Just(&b"\xff"[..]),
        Just(&b""[..]),
    ];
    (
        proptest::collection::vec((0u8..4, piece), 1..30),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(ops, old_nonl, new_nonl)| {
            let lines = ops
                .into_iter()
                .map(|(op, content)| Line {
                    // Più righe tenute che cambiate, così si formano hunk separati.
                    kind: match op {
                        0 => LineKind::Removed,
                        1 => LineKind::Added,
                        _ => LineKind::Context,
                    },
                    old_line: None,
                    new_line: None,
                    content: ByteText::from(content),
                    no_newline: false,
                })
                .collect();
            Script {
                lines: mark_no_newline(lines, old_nonl, new_nonl),
            }
        })
        .prop_filter("almeno una riga cambiata", |s| {
            s.lines.iter().any(|l| l.kind != LineKind::Context)
        })
}

/// Segna l'ultima riga di ciascun lato come senza a capo, dividendo in tolta + aggiunta una
/// riga tenuta che lo è in un lato solo.
fn mark_no_newline(mut lines: Vec<Line>, old_nonl: bool, new_nonl: bool) -> Vec<Line> {
    let last = |lines: &[Line], skip: LineKind| lines.iter().rposition(|l| l.kind != skip);
    let split = |lines: &mut Vec<Line>, i: usize, old: bool, new: bool| {
        let mut added = lines[i].clone();
        lines[i].kind = LineKind::Removed;
        lines[i].no_newline = old;
        added.kind = LineKind::Added;
        added.no_newline = new;
        lines.insert(i + 1, added);
    };
    let old_last = last(&lines, LineKind::Added);
    let new_last = last(&lines, LineKind::Removed);
    match (old_last, new_last) {
        (Some(o), Some(n)) if o == n => {
            // Riga tenuta, ultima nei due lati.
            if old_nonl == new_nonl {
                lines[o].no_newline = old_nonl;
            } else {
                split(&mut lines, o, old_nonl, new_nonl);
            }
        }
        _ => {
            // Prima la posizione più avanti, così l'altra non si sposta.
            let mut marks: Vec<(usize, bool)> = Vec::new();
            if let Some(o) = old_last {
                marks.push((o, true));
            }
            if let Some(n) = new_last {
                marks.push((n, false));
            }
            marks.sort_by_key(|m| std::cmp::Reverse(m.0));
            for (i, old_side) in marks {
                let flag = if old_side { old_nonl } else { new_nonl };
                match (lines[i].kind, old_side) {
                    (LineKind::Context, true) => split(&mut lines, i, flag, false),
                    (LineKind::Context, false) => split(&mut lines, i, false, flag),
                    _ => lines[i].no_newline = flag,
                }
            }
        }
    }
    // Una riga vuota senza a capo non esiste: sarebbe zero byte.
    for l in &mut lines {
        if l.no_newline && l.content.is_empty() {
            l.content = ByteText::from("z");
        }
    }
    canonical(lines)
}

/// Prima le tolte e poi le aggiunte in ogni gruppo, come git.
fn canonical(lines: Vec<Line>) -> Vec<Line> {
    let mut out = Vec::new();
    let mut added = Vec::new();
    for l in lines {
        match l.kind {
            LineKind::Removed => out.push(l),
            LineKind::Added => added.push(l),
            LineKind::Context => {
                out.append(&mut added);
                out.push(l);
            }
        }
    }
    out.append(&mut added);
    out
}

impl Script {
    /// Le righe con i numeri dei due lati.
    fn numbered(&self) -> Vec<Line> {
        let (mut old, mut new) = (1, 1);
        let mut lines = self.lines.clone();
        for l in &mut lines {
            if l.kind != LineKind::Added {
                l.old_line = Some(old);
                old += 1;
            }
            if l.kind != LineKind::Removed {
                l.new_line = Some(new);
                new += 1;
            }
        }
        lines
    }

    fn side(&self, skip: LineKind) -> Option<Vec<u8>> {
        let lines: Vec<&Line> = self.lines.iter().filter(|l| l.kind != skip).collect();
        (!lines.is_empty()).then(|| join(&lines))
    }

    fn old_side(&self) -> Option<Vec<u8>> {
        self.side(LineKind::Added)
    }

    fn new_side(&self) -> Option<Vec<u8>> {
        self.side(LineKind::Removed)
    }

    fn diff(&self) -> FileDiff {
        let lines = self.numbered();
        let changes: Vec<usize> = (0..lines.len())
            .filter(|&i| lines[i].kind != LineKind::Context)
            .collect();
        // Gruppi di righe cambiate a non più di 2 × CONTEXT righe l'uno dall'altro.
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for &i in &changes {
            let start = i.saturating_sub(CONTEXT);
            let end = (i + CONTEXT + 1).min(lines.len());
            match ranges.last_mut() {
                Some(r) if start <= r.1 => r.1 = end,
                _ => ranges.push((start, end)),
            }
        }
        let hunks = ranges
            .into_iter()
            .map(|(s, e)| {
                let body = lines[s..e].to_vec();
                let before = |skip| lines[..s].iter().filter(|l| l.kind != skip).count();
                let count = |skip| body.iter().filter(|l| l.kind != skip).count();
                let start = |skip| {
                    let (b, c) = (before(skip), count(skip));
                    if c == 0 { b } else { b + 1 }
                };
                Hunk {
                    old_start: start(LineKind::Added) as u32,
                    old_count: count(LineKind::Added) as u32,
                    new_start: start(LineKind::Removed) as u32,
                    new_count: count(LineKind::Removed) as u32,
                    section: ByteText::default(),
                    lines: body,
                }
            })
            .collect();
        let change = match (self.old_side(), self.new_side()) {
            (None, _) => Change::Added,
            (_, None) => Change::Deleted,
            _ => Change::Modified,
        };
        FileDiff {
            old_path: RepoPath::from("f.txt"),
            new_path: RepoPath::from("f.txt"),
            change,
            similarity: None,
            old_mode: (change != Change::Added).then_some(0o100644),
            new_mode: (change != Change::Deleted).then_some(0o100644),
            old_oid: None,
            new_oid: None,
            binary: false,
            hunks,
        }
    }

    /// Il riferimento di ogni riga del copione (`None` per quelle tenute).
    fn refs(&self) -> Vec<Option<LineRef>> {
        self.numbered().iter().map(Line::line_ref).collect()
    }

    /// Le righe attese nel risultato: quelle per cui `keep` (riga, selezionata) è vero,
    /// nell'ordine del diff con tolte e aggiunte alternate in ogni gruppo (prima tolta, prima
    /// aggiunta, seconda tolta...); una riga senza a capo che non è più l'ultima lo prende.
    fn result(&self, keep: impl Fn(&Line, bool) -> bool, sel: &[bool]) -> Vec<Line> {
        let marked: Vec<(Line, bool)> = self
            .lines
            .iter()
            .cloned()
            .zip(sel.iter().copied())
            .collect();
        let mut out: Vec<Line> = alternate(marked)
            .into_iter()
            .filter(|(l, s)| keep(l, *s))
            .map(|(l, _)| l)
            .collect();
        let n = out.len();
        for l in out.iter_mut().take(n.saturating_sub(1)) {
            l.no_newline = false;
        }
        out
    }
}

/// Alterna tolte e aggiunte in ogni gruppo di modifiche.
fn alternate(lines: Vec<(Line, bool)>) -> Vec<(Line, bool)> {
    let mut out = Vec::new();
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    let flush = |removed: &mut Vec<(Line, bool)>,
                 added: &mut Vec<(Line, bool)>,
                 out: &mut Vec<(Line, bool)>| {
        let n = removed.len().max(added.len());
        let (mut r, mut a) = (removed.drain(..), added.drain(..));
        for _ in 0..n {
            out.extend(r.next());
            out.extend(a.next());
        }
    };
    for (l, s) in lines {
        match l.kind {
            LineKind::Removed => removed.push((l, s)),
            LineKind::Added => added.push((l, s)),
            LineKind::Context => {
                flush(&mut removed, &mut added, &mut out);
                out.push((l, s));
            }
        }
    }
    flush(&mut removed, &mut added, &mut out);
    out
}

fn join(lines: &[&Line]) -> Vec<u8> {
    let mut out = Vec::new();
    for l in lines {
        out.extend_from_slice(l.content.as_bytes());
        if !l.no_newline {
            out.push(b'\n');
        }
    }
    out
}

/// Righe di un contenuto, ognuna con il suo a capo (l'ultima può non averlo).
fn split_lines(content: &[u8]) -> Vec<&[u8]> {
    content.split_inclusive(|&b| b == b'\n').collect()
}

/// Aggiunge una riga al risultato: dopo una riga senza a capo non può venirne un'altra.
fn push(out: &mut Vec<u8>, line: &[u8]) -> Result<(), String> {
    if out.last().is_some_and(|&b| b != b'\n') {
        return Err("riga senza a capo seguita da un'altra riga".into());
    }
    out.extend_from_slice(line);
    Ok(())
}

/// Applica `patch` a `base` (`None` = file assente) come `git apply` senza `--recount`:
/// controlla intestazioni, conteggi, numeri di riga e contesto. `None` nel risultato = file
/// eliminato.
fn apply(base: Option<&[u8]>, patch: &FileDiff) -> Result<Option<Vec<u8>>, String> {
    let base = match (base, patch.change) {
        (Some(_), Change::Added) => return Err("file nuovo, ma esiste già".into()),
        (None, Change::Added) => Vec::new(),
        (None, _) => return Err("il file non esiste".into()),
        (Some(b), _) => b.to_vec(),
    };
    let src = split_lines(&base);
    let mut out: Vec<u8> = Vec::new();
    let mut out_lines = 0usize;
    let mut pos = 0usize;
    for h in &patch.hunks {
        let first = if h.old_count == 0 {
            h.old_start as usize
        } else {
            (h.old_start as usize).checked_sub(1).ok_or("old_start 0")?
        };
        if first < pos || first > src.len() {
            return Err(format!("hunk fuori posto: {first} (cursore {pos})"));
        }
        for l in &src[pos..first] {
            push(&mut out, l)?;
            out_lines += 1;
        }
        pos = first;
        let expected_new = if h.new_count == 0 {
            out_lines
        } else {
            out_lines + 1
        };
        if h.new_start as usize != expected_new {
            return Err(format!(
                "new_start {} invece di {expected_new}",
                h.new_start
            ));
        }
        let (mut old_n, mut new_n) = (0, 0);
        for l in &h.lines {
            let mut bytes = l.content.as_bytes().to_vec();
            if !l.no_newline {
                bytes.push(b'\n');
            }
            if l.kind != LineKind::Added {
                if src.get(pos) != Some(&&bytes[..]) {
                    return Err(format!("contesto diverso alla riga {}", pos + 1));
                }
                if l.old_line != Some(pos as u32 + 1) {
                    return Err("numero di riga vecchio sbagliato".into());
                }
                pos += 1;
                old_n += 1;
            }
            if l.kind != LineKind::Removed {
                push(&mut out, &bytes)?;
                out_lines += 1;
                new_n += 1;
                if l.new_line != Some(out_lines as u32) {
                    return Err("numero di riga nuovo sbagliato".into());
                }
            }
        }
        if (old_n, new_n) != (h.old_count, h.new_count) {
            return Err(format!(
                "conteggi {}/{} invece di {old_n}/{new_n}",
                h.old_count, h.new_count
            ));
        }
    }
    for l in &src[pos..] {
        push(&mut out, l)?;
    }
    if patch.change == Change::Deleted {
        if !out.is_empty() {
            return Err("eliminazione che lascia contenuto".into());
        }
        return Ok(None);
    }
    Ok(Some(out))
}

/// Applica la patch costruita, oppure lascia `base` se non c'è patch.
fn applied(
    base: Option<Vec<u8>>,
    f: &FileDiff,
    refs: &[LineRef],
    dir: PatchDirection,
) -> Result<Option<Vec<u8>>, String> {
    match build_patch(f, refs, dir).map_err(|e| e.to_string())? {
        None => Ok(base),
        Some(p) => {
            let bytes = p.to_patch();
            let reread = parse_diff(&bytes).map_err(|e| e.to_string())?;
            if reread.files != [p.clone()] {
                return Err("la patch non si rilegge identica".into());
            }
            apply(base.as_deref(), &p)
        }
    }
}

fn chosen(script: &Script, sel: &[bool]) -> Vec<LineRef> {
    script
        .refs()
        .iter()
        .zip(sel)
        .filter_map(|(r, s)| r.filter(|_| *s))
        .collect()
}

fn bytes_of(lines: &[Line], absent: bool) -> Option<Vec<u8>> {
    if absent {
        return None;
    }
    let refs: Vec<&Line> = lines.iter().collect();
    Some(join(&refs))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Il copione stesso è coerente: il diff generato porta dal vecchio al nuovo.
    #[test]
    fn il_generatore_produce_diff_validi(script in script_strategy()) {
        let f = script.diff();
        prop_assert_eq!(apply(script.old_side().as_deref(), &f), Ok(script.new_side()));
    }

    /// Stage: le righe selezionate entrano, le altre no; selezione totale = diff originale;
    /// selezione vuota = nessuna patch.
    #[test]
    fn stage_applica_solo_le_righe_scelte(
        script in script_strategy(),
        bits in proptest::collection::vec(any::<bool>(), 64),
    ) {
        let f = script.diff();
        let sel: Vec<bool> = (0..script.lines.len()).map(|i| bits[i % 64]).collect();
        let refs = chosen(&script, &sel);
        let target = script.result(
            |l, s| match l.kind {
                LineKind::Context => true,
                LineKind::Removed => !s,
                LineKind::Added => s,
            },
            &sel,
        );
        let all_removed = f.change == Change::Deleted && target.is_empty();
        let expected = if refs.is_empty() {
            script.old_side()
        } else {
            bytes_of(&target, all_removed)
        };
        prop_assert_eq!(applied(script.old_side(), &f, &refs, Stage), Ok(expected));
        prop_assert_eq!(build_patch(&f, &[], Stage), Ok(None));

        let all = f.line_refs();
        let total = build_patch(&f, &all, Stage).unwrap().unwrap();
        prop_assert_eq!(&total.hunks, &f.hunks);
        prop_assert_eq!(total.change, f.change);
    }

    /// Unstage: le righe selezionate escono dall'indice, le altre restano; dà lo stesso
    /// contenuto dello stage delle righe non selezionate partendo dal lato vecchio.
    #[test]
    fn unstage_toglie_solo_le_righe_scelte(
        script in script_strategy(),
        bits in proptest::collection::vec(any::<bool>(), 64),
    ) {
        let f = script.diff();
        let sel: Vec<bool> = (0..script.lines.len()).map(|i| bits[i % 64]).collect();
        let refs = chosen(&script, &sel);
        let target = script.result(
            |l, s| match l.kind {
                LineKind::Context => true,
                LineKind::Removed => s,
                LineKind::Added => !s,
            },
            &sel,
        );
        let all_removed = f.change == Change::Added && target.is_empty();
        let expected = if refs.is_empty() {
            script.new_side()
        } else {
            bytes_of(&target, all_removed)
        };
        let unstaged = applied(script.new_side(), &f, &refs, Unstage);
        prop_assert_eq!(&unstaged, &Ok(expected));

        // Lo stesso contenuto dello stage del complemento (se il complemento non è vuoto).
        let others: Vec<LineRef> = f.line_refs().into_iter().filter(|r| !refs.contains(r)).collect();
        if !others.is_empty() && !refs.is_empty() {
            prop_assert_eq!(&applied(script.old_side(), &f, &others, Stage), &unstaged);
        }
        // Unstage totale: si torna al lato vecchio.
        prop_assert_eq!(applied(script.new_side(), &f, &f.line_refs(), Unstage), Ok(script.old_side()));
    }

    /// Stage e poi unstage della stessa selezione (nel diff dell'indice, che dopo lo stage è
    /// la patch stessa) riportano al contenuto iniziale.
    #[test]
    fn stage_poi_unstage_torna_all_inizio(
        script in script_strategy(),
        bits in proptest::collection::vec(any::<bool>(), 64),
    ) {
        let f = script.diff();
        let sel: Vec<bool> = (0..script.lines.len()).map(|i| bits[i % 64]).collect();
        let refs = chosen(&script, &sel);
        if refs.is_empty() {
            return Ok(());
        }
        let staged = build_patch(&f, &refs, Stage).unwrap().unwrap();
        let index = apply(script.old_side().as_deref(), &staged).unwrap();
        let back = applied(index, &staged, &staged.line_refs(), Unstage);
        prop_assert_eq!(back, Ok(script.old_side()));
    }
}
