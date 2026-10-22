//! Test delle corsie del grafo: casi fissi, un log reale di git e test di proprietà.

use std::collections::BTreeSet;

use proptest::prelude::*;

use super::*;

/// Oid sintetico riconoscibile: tutti i byte valgono `n`, tranne i primi due che portano
/// l'indice (così si possono creare più di 256 commit distinti).
fn id(n: u16) -> Oid {
    let mut bytes = [0xab; 20];
    bytes[..2].copy_from_slice(&n.to_be_bytes());
    Oid::from_bytes(bytes)
}

/// Grafo descritto come `(commit, genitori)`, in ordine topologico dal più recente.
fn layout(nodes: &[(u16, &[u16])]) -> GraphLayout {
    let owned: Vec<(Oid, Vec<Oid>)> = nodes
        .iter()
        .map(|(c, ps)| (id(*c), ps.iter().map(|p| id(*p)).collect()))
        .collect();
    let layout = layout_graph_nodes(owned.iter().map(|(c, ps)| (c, ps.as_slice())));
    check_invariants(&owned, &layout);
    layout
}

fn seg(from: usize, to: usize, lane: usize) -> GraphSegment {
    GraphSegment { from, to, lane }
}

fn columns(layout: &GraphLayout) -> Vec<usize> {
    layout.rows.iter().map(|r| r.column).collect()
}

fn segments(layout: &GraphLayout) -> Vec<Vec<GraphSegment>> {
    layout.rows.iter().map(|r| r.segments.clone()).collect()
}

// --- Casi fissi -------------------------------------------------------------------------

#[test]
fn nessun_commit() {
    assert_eq!(layout(&[]), GraphLayout::default());
}

#[test]
fn storia_lineare_in_una_colonna() {
    let g = layout(&[(3, &[2]), (2, &[1]), (1, &[])]);
    assert_eq!(columns(&g), [0, 0, 0]);
    assert_eq!(
        segments(&g),
        [vec![seg(0, 0, 0)], vec![seg(0, 0, 0)], vec![]]
    );
    assert_eq!(g.width, 1);
}

#[test]
fn due_branch_convergono_sulla_base() {
    // d e c partono da b.
    let g = layout(&[(4, &[2]), (3, &[2]), (2, &[1]), (1, &[])]);
    assert_eq!(columns(&g), [0, 1, 0, 0]);
    assert_eq!(
        segments(&g),
        [
            vec![seg(0, 0, 0)],
            vec![seg(0, 0, 0), seg(1, 0, 1)],
            vec![seg(0, 0, 0)],
            vec![],
        ]
    );
    assert_eq!(g.width, 2);
}

#[test]
fn merge_apre_una_diramazione() {
    // m unisce a (primo genitore) e f; f parte da a.
    let g = layout(&[(9, &[1, 5]), (5, &[1]), (1, &[])]);
    assert_eq!(columns(&g), [0, 1, 0]);
    assert_eq!(
        segments(&g),
        [
            vec![seg(0, 0, 0), seg(0, 1, 1)],
            vec![seg(0, 0, 0), seg(1, 0, 1)],
            vec![],
        ]
    );
    assert_eq!(g.width, 2);
}

#[test]
fn octopus_merge_con_tre_genitori() {
    // m unisce p1, p2, p3, che partono tutti da r.
    let g = layout(&[(9, &[1, 2, 3]), (3, &[0]), (2, &[0]), (1, &[0]), (0, &[])]);
    assert_eq!(columns(&g), [0, 2, 1, 0, 0]);
    assert_eq!(
        segments(&g),
        [
            vec![seg(0, 0, 0), seg(0, 1, 1), seg(0, 2, 2)],
            vec![seg(0, 0, 0), seg(1, 1, 1), seg(2, 2, 2)],
            vec![seg(0, 0, 0), seg(1, 1, 1), seg(2, 2, 2)],
            vec![seg(0, 0, 0), seg(1, 0, 1), seg(2, 0, 2)],
            vec![],
        ]
    );
    assert_eq!(g.width, 3);
}

#[test]
fn merge_su_un_genitore_gia_atteso_riusa_la_sua_corsia() {
    // t (colonna 0) aspetta a; m (colonna 1) unisce b e a: il secondo genitore va sulla
    // corsia 0, che aspetta già a, senza aprirne una nuova.
    let g = layout(&[(7, &[1]), (8, &[2, 1]), (2, &[1]), (1, &[])]);
    assert_eq!(columns(&g), [0, 1, 1, 0]);
    assert_eq!(
        segments(&g),
        [
            vec![seg(0, 0, 0)],
            vec![seg(0, 0, 0), seg(1, 0, 0), seg(1, 1, 1)],
            vec![seg(0, 0, 0), seg(1, 0, 1)],
            vec![],
        ]
    );
    assert_eq!(g.width, 2);
}

#[test]
fn piu_radici() {
    // Due storie indipendenti intrecciate: x1→x0 e y1→y0.
    let g = layout(&[(11, &[10]), (21, &[20]), (10, &[]), (20, &[])]);
    assert_eq!(columns(&g), [0, 1, 0, 1]);
    assert_eq!(
        segments(&g),
        [
            vec![seg(0, 0, 0)],
            vec![seg(0, 0, 0), seg(1, 1, 1)],
            vec![seg(1, 1, 1)],
            vec![],
        ]
    );
    assert_eq!(g.width, 2);
}

#[test]
fn una_radice_libera_la_corsia_per_il_commit_successivo() {
    // a→b; c è una radice a sé; dopo c la corsia 1 si libera e d (nuova punta) la riusa.
    let g = layout(&[(1, &[2]), (3, &[]), (4, &[2]), (2, &[])]);
    assert_eq!(columns(&g), [0, 1, 1, 0]);
    assert_eq!(g.width, 2);
}

#[test]
fn genitore_assente_resta_sospeso() {
    // Log troncato: il genitore 0 non compare.
    let g = layout(&[(2, &[1]), (1, &[0])]);
    assert_eq!(segments(&g), [vec![seg(0, 0, 0)], vec![seg(0, 0, 0)]]);
}

#[test]
fn genitori_duplicati_disegnati_una_volta() {
    let g = layout(&[(2, &[1, 1]), (1, &[])]);
    assert_eq!(segments(&g), [vec![seg(0, 0, 0)], vec![]]);
}

#[test]
fn layout_graph_usa_id_e_genitori_dei_commit() {
    let sig = Signature {
        name: "T".into(),
        email: "t@e".into(),
        time: 0,
        offset_min: 0,
    };
    let commit = |c: u16, ps: &[u16]| Commit {
        id: id(c),
        parents: ps.iter().map(|p| id(*p)).collect(),
        author: sig.clone(),
        committer: sig.clone(),
        summary: String::new(),
        body: String::new(),
    };
    let commits = [commit(9, &[1, 5]), commit(5, &[1]), commit(1, &[])];
    assert_eq!(
        layout_graph(&commits),
        layout(&[(9, &[1, 5]), (5, &[1]), (1, &[])])
    );
}

#[test]
fn serde_andata_e_ritorno() {
    let g = layout(&[(9, &[1, 5]), (5, &[1]), (1, &[])]);
    let json = serde_json::to_string(&g).expect("serializza");
    let back: GraphLayout = serde_json::from_str(&json).expect("deserializza");
    assert_eq!(back, g);
}

// --- Log reale ---------------------------------------------------------------------------

/// Output reale di git (2.43.0) su un repo con branch, merge, octopus merge di tre branch e
/// una seconda radice (`--orphan`):
/// `git log --all --topo-order --format='%H %P' > log-topo-parents.txt`.
///
/// `git log --all --topo-order --graph --format=%s` sullo stesso repo:
/// ```text
/// * d
/// *---.   octopus
/// |\ \ \
/// | | | * x3
/// | |_|/
/// |/| |
/// | | * x2
/// | |/
/// |/|
/// | * x1
/// |/
/// *   merge feat
/// |\
/// | * f2
/// | * f1
/// * | c
/// |/
/// * b
/// * a
/// * r2
/// * r1
/// ```
const LOG_TOPO: &[u8] = include_bytes!("testdata/log-topo-parents.txt");

/// Legge le righe `<oid> <genitori...>` di `git log --format='%H %P'`.
fn parse_parents(bytes: &[u8]) -> Vec<(Oid, Vec<Oid>)> {
    bytes
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut oids = line
                .split(|&b| b == b' ')
                .filter(|f| !f.is_empty())
                .map(|f| Oid::from_hex(f).expect("oid valido nel file di esempio"));
            let id = oids.next().expect("riga con almeno un oid");
            (id, oids.collect())
        })
        .collect()
}

#[test]
fn corsie_di_un_log_reale() {
    let nodes = parse_parents(LOG_TOPO);
    assert_eq!(nodes.len(), 13);
    let g = layout_graph_nodes(nodes.iter().map(|(c, ps)| (c, ps.as_slice())));
    check_invariants(&nodes, &g);
    // d, octopus, x3, x2, x1, merge feat, f2, f1, c, b, a, r2, r1
    assert_eq!(columns(&g), [0, 0, 3, 2, 1, 0, 1, 1, 0, 0, 0, 0, 0]);
    assert_eq!(g.width, 4);
    // L'octopus apre tre diramazioni.
    assert_eq!(
        g.rows[1].segments,
        [seg(0, 0, 0), seg(0, 1, 1), seg(0, 2, 2), seg(0, 3, 3)]
    );
    // Le quattro corsie convergono sul merge di feat.
    assert_eq!(
        g.rows[4].segments,
        [seg(0, 0, 0), seg(1, 0, 1), seg(2, 0, 2), seg(3, 0, 3)]
    );
    // La radice `a` chiude tutto: nessun tratto verso la seconda storia.
    assert!(g.rows[10].segments.is_empty());
}

/// Output reale di [`LOG_ARGS`] (generato da `testdata/log.sh`), letto con [`parse_log`]:
/// le corsie si calcolano direttamente sui [`Commit`] del parser.
#[test]
fn corsie_dei_commit_di_parse_log() {
    let commits = parse_log(include_bytes!("testdata/log.bin")).expect("log di esempio valido");
    assert!(
        commits.iter().any(|c| c.parents.len() > 1),
        "il log contiene un merge"
    );
    let g = layout_graph(&commits);
    let nodes: Vec<(Oid, Vec<Oid>)> = commits.iter().map(|c| (c.id, c.parents.clone())).collect();
    check_invariants(&nodes, &g);
    assert!(g.width >= 2, "il merge apre una seconda corsia");
}

// --- Invarianti --------------------------------------------------------------------------

/// Dove finisce un arco seguito tratto per tratto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum End {
    /// Nel commit della riga indicata.
    Row(usize),
    /// Oltre l'ultima riga (genitore assente dall'input).
    Dangling,
}

/// Verifica gli invarianti di §3 leggendo solo il risultato:
/// - una riga per commit, colonne dentro `width`, `width` minima;
/// - dal commit escono tanti tratti quanti genitori distinti (nessuna corsia passa sotto il
///   commit, quindi nessun commit occupa due colonne né ne copre un'altra);
/// - ogni arco genitore–figlio è disegnato una sola volta: seguendo i tratti si arriva
///   esattamente ai genitori, ognuno una volta;
/// - continuità: ogni tratto che non entra in un commit prosegue in un solo tratto, e ogni
///   tratto è raggiungibile da un commit;
/// - riuso delle corsie libere: una nuova punta e ogni nuova diramazione prendono le colonne
///   libere più a sinistra; la larghezza è limitata dal numero di archi aperti.
fn check_invariants(nodes: &[(Oid, Vec<Oid>)], layout: &GraphLayout) {
    let rows = &layout.rows;
    assert_eq!(rows.len(), nodes.len(), "una riga per commit");

    let max_col = rows
        .iter()
        .flat_map(|r| {
            std::iter::once(r.column).chain(r.segments.iter().flat_map(|s| [s.from, s.to]))
        })
        .max();
    assert_eq!(
        layout.width,
        max_col.map_or(0, |m| m + 1),
        "larghezza minima"
    );

    let index: std::collections::HashMap<Oid, usize> = nodes
        .iter()
        .enumerate()
        .rev()
        .map(|(i, (c, _))| (*c, i))
        .collect();

    let mut visited: Vec<Vec<bool>> = rows.iter().map(|r| vec![false; r.segments.len()]).collect();

    for (i, row) in rows.iter().enumerate() {
        // Tratti che passano accanto: una sola partenza per colonna.
        let through: Vec<usize> = row
            .segments
            .iter()
            .filter(|s| s.from != row.column)
            .map(|s| s.from)
            .collect();
        let unique: BTreeSet<usize> = through.iter().copied().collect();
        assert_eq!(unique.len(), through.len(), "riga {i}: corsie sovrapposte");
        for s in &row.segments {
            assert!(s.lane == s.from || s.lane == s.to, "riga {i}: corsia {s:?}");
        }

        // Continuità con la riga precedente.
        if i > 0 {
            let arriving: BTreeSet<usize> = rows[i - 1]
                .segments
                .iter()
                .map(|s| s.to)
                .filter(|&t| t != row.column)
                .collect();
            assert_eq!(
                arriving, unique,
                "riga {i}: tratti interrotti o senza origine"
            );
        }

        // Archi uscenti dal commit: uno per genitore distinto, ciascuno arriva al genitore.
        let mut parents: Vec<Oid> = Vec::new();
        for p in &nodes[i].1 {
            if !parents.contains(p) {
                parents.push(*p);
            }
        }
        let mut expected: Vec<End> = parents
            .iter()
            .map(|p| match index.get(p) {
                Some(&j) if j > i => End::Row(j),
                _ => End::Dangling,
            })
            .collect();
        let mut reached: Vec<End> = Vec::new();
        for (k, s) in row.segments.iter().enumerate() {
            if s.from == row.column {
                reached.push(follow(rows, i, k, &mut visited));
            }
        }
        expected.sort();
        reached.sort();
        assert_eq!(reached, expected, "riga {i}: archi verso i genitori");
    }

    for (i, v) in visited.iter().enumerate() {
        assert!(
            v.iter().all(|&x| x),
            "riga {i}: tratto non raggiungibile da un commit"
        );
    }

    check_lane_reuse(layout);
}

/// Segue un arco a partire dal tratto `k` della riga `i` fino al commit in cui entra.
fn follow(rows: &[GraphRow], mut i: usize, mut k: usize, visited: &mut [Vec<bool>]) -> End {
    loop {
        visited[i][k] = true;
        let to = rows[i].segments[k].to;
        let Some(next) = rows.get(i + 1) else {
            return End::Dangling;
        };
        if to == next.column {
            return End::Row(i + 1);
        }
        i += 1;
        k = next
            .segments
            .iter()
            .position(|s| s.from == to && s.from != next.column)
            .unwrap_or_else(|| {
                panic!("riga {i}: il tratto che arriva in colonna {to} non prosegue")
            });
    }
}

/// Le colonne libere vengono riusate da sinistra e la larghezza resta limitata.
fn check_lane_reuse(layout: &GraphLayout) {
    let rows = &layout.rows;
    let mut max_open = 0;
    for (i, row) in rows.iter().enumerate() {
        max_open = max_open.max(row.segments.len());

        // Colonne occupate quando arriva il commit: dove arrivano i tratti della riga sopra.
        let incoming: BTreeSet<usize> = match i {
            0 => BTreeSet::new(),
            _ => rows[i - 1].segments.iter().map(|s| s.to).collect(),
        };
        if !incoming.contains(&row.column) {
            // Nuova punta: prima colonna libera.
            let first_free = (0..).find(|c| !incoming.contains(c)).unwrap_or(0);
            assert_eq!(
                row.column, first_free,
                "riga {i}: nuova punta non nella prima colonna libera"
            );
        } else {
            // Colonna del commit: la prima delle corsie che lo aspettavano.
            let first_waiting = rows[i - 1]
                .segments
                .iter()
                .filter(|s| s.to == row.column)
                .map(|s| s.lane)
                .min();
            assert_eq!(
                Some(row.column),
                first_waiting,
                "riga {i}: colonna del commit"
            );
        }

        // Nuove diramazioni: uscenti su corsie diverse da quella del commit e non già aperte.
        let through: BTreeSet<usize> = row
            .segments
            .iter()
            .filter(|s| s.from != row.column)
            .map(|s| s.from)
            .collect();
        let has_parents = row.segments.iter().any(|s| s.from == row.column);
        let new_lanes: BTreeSet<usize> = row
            .segments
            .iter()
            .filter(|s| s.from == row.column && s.lane != row.column && !through.contains(&s.lane))
            .map(|s| s.lane)
            .collect();
        let free: Vec<usize> = (0..)
            .filter(|c| !through.contains(c) && !(has_parents && *c == row.column))
            .take(new_lanes.len())
            .collect();
        assert_eq!(
            new_lanes.into_iter().collect::<Vec<_>>(),
            free,
            "riga {i}: diramazioni non nelle prime colonne libere"
        );
    }
    assert!(
        layout.width <= max_open + 1,
        "larghezza {} oltre gli archi aperti ({max_open}) + 1",
        layout.width
    );
}

// --- Test di proprietà ---------------------------------------------------------------------

/// Grafo casuale in ordine topologico: ogni commit ha da 0 a 4 genitori scelti tra i commit
/// successivi; qualche genitore manca dall'input (log troncato).
fn arb_graph() -> impl Strategy<Value = Vec<(Oid, Vec<Oid>)>> {
    prop::collection::vec(prop::collection::vec((any::<u16>(), 0u8..12), 0..5), 0..60).prop_map(
        |spec| {
            let n = spec.len();
            spec.into_iter()
                .enumerate()
                .map(|(i, parents)| {
                    let parents = parents
                        .into_iter()
                        .map(|(r, missing)| {
                            let later = n - i - 1;
                            if missing == 0 || later == 0 {
                                id(10_000 + r % 100)
                            } else {
                                id((i + 1 + usize::from(r) % later) as u16)
                            }
                        })
                        .collect();
                    (id(i as u16), parents)
                })
                .collect()
        },
    )
}

/// Come [`arb_graph`] ma con storie "realistiche": pochi merge, spesso il genitore è il commit
/// subito dopo, così le corsie restano aperte a lungo e si riusano.
fn arb_history() -> impl Strategy<Value = Vec<(Oid, Vec<Oid>)>> {
    prop::collection::vec((0u8..10, any::<u16>(), any::<u16>()), 1..80).prop_map(|spec| {
        let n = spec.len();
        spec.into_iter()
            .enumerate()
            .map(|(i, (kind, a, b))| {
                let later = n - i - 1;
                let pick = |r: u16| id((i + 1 + usize::from(r) % later.max(1)) as u16);
                let parents = match (kind, later) {
                    (_, 0) | (0, _) => vec![],
                    (1 | 2, _) => vec![pick(a), pick(b)],
                    (3, _) => vec![pick(a)],
                    _ => vec![id((i + 1) as u16)],
                };
                (id(i as u16), parents)
            })
            .collect()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn invarianti_su_grafi_casuali(nodes in arb_graph()) {
        let g = layout_graph_nodes(nodes.iter().map(|(c, ps)| (c, ps.as_slice())));
        check_invariants(&nodes, &g);
    }

    #[test]
    fn invarianti_su_storie_casuali(nodes in arb_history()) {
        let g = layout_graph_nodes(nodes.iter().map(|(c, ps)| (c, ps.as_slice())));
        check_invariants(&nodes, &g);
    }

    #[test]
    fn deterministico(nodes in arb_graph()) {
        let a = layout_graph_nodes(nodes.iter().map(|(c, ps)| (c, ps.as_slice())));
        let b = layout_graph_nodes(nodes.iter().map(|(c, ps)| (c, ps.as_slice())));
        prop_assert_eq!(a, b);
    }
}
