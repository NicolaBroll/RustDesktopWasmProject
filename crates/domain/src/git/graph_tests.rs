//! Graph lane tests: fixed cases, a real git log and property tests.

use std::collections::BTreeSet;

use proptest::prelude::*;

use super::*;

/// Recognizable synthetic Oid: all bytes are `n`, except the first two which carry
/// the index (so more than 256 distinct commits can be created).
fn id(n: u16) -> Oid {
    let mut bytes = [0xab; 20];
    bytes[..2].copy_from_slice(&n.to_be_bytes());
    Oid::from_bytes(bytes)
}

/// Graph described as `(commit, parents)`, in topological order, most recent first.
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

// --- Fixed cases ------------------------------------------------------------------------

#[test]
fn no_commits() {
    assert_eq!(layout(&[]), GraphLayout::default());
}

#[test]
fn linear_history_in_one_column() {
    let g = layout(&[(3, &[2]), (2, &[1]), (1, &[])]);
    assert_eq!(columns(&g), [0, 0, 0]);
    assert_eq!(
        segments(&g),
        [vec![seg(0, 0, 0)], vec![seg(0, 0, 0)], vec![]]
    );
    assert_eq!(g.width, 1);
}

#[test]
fn two_branches_converge_on_the_base() {
    // d and c start from b.
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
fn merge_opens_a_fork() {
    // m merges a (first parent) and f; f starts from a.
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
fn octopus_merge_with_three_parents() {
    // m merges p1, p2, p3, which all start from r.
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
fn merge_on_an_already_awaited_parent_reuses_its_lane() {
    // t (column 0) waits for a; m (column 1) merges b and a: the second parent goes on
    // lane 0, which already waits for a, without opening a new one.
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
fn several_roots() {
    // Two independent interleaved histories: x1→x0 and y1→y0.
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
fn a_root_frees_its_lane_for_the_next_commit() {
    // a→b; c is a root on its own; after c lane 1 becomes free and d (new tip) reuses it.
    let g = layout(&[(1, &[2]), (3, &[]), (4, &[2]), (2, &[])]);
    assert_eq!(columns(&g), [0, 1, 1, 0]);
    assert_eq!(g.width, 2);
}

#[test]
fn missing_parent_stays_dangling() {
    // Truncated log: parent 0 does not appear.
    let g = layout(&[(2, &[1]), (1, &[0])]);
    assert_eq!(segments(&g), [vec![seg(0, 0, 0)], vec![seg(0, 0, 0)]]);
}

#[test]
fn duplicate_parents_drawn_once() {
    let g = layout(&[(2, &[1, 1]), (1, &[])]);
    assert_eq!(segments(&g), [vec![seg(0, 0, 0)], vec![]]);
}

#[test]
fn layout_graph_uses_commit_ids_and_parents() {
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
fn serde_round_trip() {
    let g = layout(&[(9, &[1, 5]), (5, &[1]), (1, &[])]);
    let json = serde_json::to_string(&g).expect("serializes");
    let back: GraphLayout = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back, g);
}

// --- Real log ----------------------------------------------------------------------------

/// Real git output (2.43.0) on a repo with branches, merges, an octopus merge of three branches and
/// a second root (`--orphan`):
/// `git log --all --topo-order --format='%H %P' > log-topo-parents.txt`.
///
/// `git log --all --topo-order --graph --format=%s` on the same repo:
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

/// Reads the `<oid> <parents...>` lines of `git log --format='%H %P'`.
fn parse_parents(bytes: &[u8]) -> Vec<(Oid, Vec<Oid>)> {
    bytes
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut oids = line
                .split(|&b| b == b' ')
                .filter(|f| !f.is_empty())
                .map(|f| Oid::from_hex(f).expect("valid oid in the sample file"));
            let id = oids.next().expect("line with at least one oid");
            (id, oids.collect())
        })
        .collect()
}

#[test]
fn lanes_of_a_real_log() {
    let nodes = parse_parents(LOG_TOPO);
    assert_eq!(nodes.len(), 13);
    let g = layout_graph_nodes(nodes.iter().map(|(c, ps)| (c, ps.as_slice())));
    check_invariants(&nodes, &g);
    // d, octopus, x3, x2, x1, merge feat, f2, f1, c, b, a, r2, r1
    assert_eq!(columns(&g), [0, 0, 3, 2, 1, 0, 1, 1, 0, 0, 0, 0, 0]);
    assert_eq!(g.width, 4);
    // The octopus opens three forks.
    assert_eq!(
        g.rows[1].segments,
        [seg(0, 0, 0), seg(0, 1, 1), seg(0, 2, 2), seg(0, 3, 3)]
    );
    // The four lanes converge on the merge of feat.
    assert_eq!(
        g.rows[4].segments,
        [seg(0, 0, 0), seg(1, 0, 1), seg(2, 0, 2), seg(3, 0, 3)]
    );
    // The root `a` closes everything: no segment towards the second history.
    assert!(g.rows[10].segments.is_empty());
}

/// Real output of [`LOG_ARGS`] (generated by `testdata/log.sh`), read with [`parse_log`]:
/// the lanes are computed directly on the parser's [`Commit`]s.
#[test]
fn lanes_of_parse_log_commits() {
    let commits = parse_log(include_bytes!("testdata/log.bin")).expect("valid sample log");
    assert!(
        commits.iter().any(|c| c.parents.len() > 1),
        "the log contains a merge"
    );
    let g = layout_graph(&commits);
    let nodes: Vec<(Oid, Vec<Oid>)> = commits.iter().map(|c| (c.id, c.parents.clone())).collect();
    check_invariants(&nodes, &g);
    assert!(g.width >= 2, "the merge opens a second lane");
}

// --- Invariants --------------------------------------------------------------------------

/// Where an edge ends up when followed segment by segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum End {
    /// In the commit of the given row.
    Row(usize),
    /// Past the last row (parent absent from the input).
    Dangling,
}

/// Checks the invariants of §3 by reading only the result:
/// - one row per commit, columns within `width`, minimal `width`;
/// - from the commit come out as many segments as distinct parents (no lane passes under the
///   commit, so no commit occupies two columns nor covers another);
/// - every parent–child edge is drawn exactly once: following the segments leads
///   exactly to the parents, each once;
/// - continuity: every segment that does not enter a commit continues in a single segment, and every
///   segment is reachable from a commit;
/// - reuse of free lanes: a new tip and every new fork take the leftmost
///   free columns; the width is bounded by the number of open edges.
fn check_invariants(nodes: &[(Oid, Vec<Oid>)], layout: &GraphLayout) {
    let rows = &layout.rows;
    assert_eq!(rows.len(), nodes.len(), "one row per commit");

    let max_col = rows
        .iter()
        .flat_map(|r| {
            std::iter::once(r.column).chain(r.segments.iter().flat_map(|s| [s.from, s.to]))
        })
        .max();
    assert_eq!(layout.width, max_col.map_or(0, |m| m + 1), "minimal width");

    let index: std::collections::HashMap<Oid, usize> = nodes
        .iter()
        .enumerate()
        .rev()
        .map(|(i, (c, _))| (*c, i))
        .collect();

    let mut visited: Vec<Vec<bool>> = rows.iter().map(|r| vec![false; r.segments.len()]).collect();

    for (i, row) in rows.iter().enumerate() {
        // Segments passing next to it: a single start per column.
        let through: Vec<usize> = row
            .segments
            .iter()
            .filter(|s| s.from != row.column)
            .map(|s| s.from)
            .collect();
        let unique: BTreeSet<usize> = through.iter().copied().collect();
        assert_eq!(unique.len(), through.len(), "row {i}: overlapping lanes");
        for s in &row.segments {
            assert!(s.lane == s.from || s.lane == s.to, "row {i}: lane {s:?}");
        }

        // Continuity with the previous row.
        if i > 0 {
            let arriving: BTreeSet<usize> = rows[i - 1]
                .segments
                .iter()
                .map(|s| s.to)
                .filter(|&t| t != row.column)
                .collect();
            assert_eq!(
                arriving, unique,
                "row {i}: broken segments or without an origin"
            );
        }

        // Edges leaving the commit: one per distinct parent, each arrives at the parent.
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
        assert_eq!(reached, expected, "row {i}: edges towards the parents");
    }

    for (i, v) in visited.iter().enumerate() {
        assert!(
            v.iter().all(|&x| x),
            "row {i}: segment not reachable from a commit"
        );
    }

    check_lane_reuse(layout);
}

/// Follows an edge starting from segment `k` of row `i` to the commit it enters.
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
                panic!("row {i}: the segment arriving at column {to} does not continue")
            });
    }
}

/// Free columns are reused from the left and the width stays bounded.
fn check_lane_reuse(layout: &GraphLayout) {
    let rows = &layout.rows;
    let mut max_open = 0;
    for (i, row) in rows.iter().enumerate() {
        max_open = max_open.max(row.segments.len());

        // Columns occupied when the commit arrives: where the segments of the row above arrive.
        let incoming: BTreeSet<usize> = match i {
            0 => BTreeSet::new(),
            _ => rows[i - 1].segments.iter().map(|s| s.to).collect(),
        };
        if !incoming.contains(&row.column) {
            // New tip: first free column.
            let first_free = (0..).find(|c| !incoming.contains(c)).unwrap_or(0);
            assert_eq!(
                row.column, first_free,
                "row {i}: new tip not in the first free column"
            );
        } else {
            // Column of the commit: the first of the lanes that were waiting for it.
            let first_waiting = rows[i - 1]
                .segments
                .iter()
                .filter(|s| s.to == row.column)
                .map(|s| s.lane)
                .min();
            assert_eq!(
                Some(row.column),
                first_waiting,
                "row {i}: column of the commit"
            );
        }

        // New forks: leaving on lanes different from the commit's and not already open.
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
            "row {i}: forks not in the first free columns"
        );
    }
    assert!(
        layout.width <= max_open + 1,
        "width {} beyond the open edges ({max_open}) + 1",
        layout.width
    );
}

// --- Property tests ------------------------------------------------------------------------

/// Random graph in topological order: each commit has 0 to 4 parents chosen among the following
/// commits; some parent is missing from the input (truncated log).
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

/// Like [`arb_graph`] but with "realistic" histories: few merges, the parent is often the commit
/// right after, so lanes stay open for a long time and get reused.
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
    fn invariants_on_random_graphs(nodes in arb_graph()) {
        let g = layout_graph_nodes(nodes.iter().map(|(c, ps)| (c, ps.as_slice())));
        check_invariants(&nodes, &g);
    }

    #[test]
    fn invariants_on_random_histories(nodes in arb_history()) {
        let g = layout_graph_nodes(nodes.iter().map(|(c, ps)| (c, ps.as_slice())));
        check_invariants(&nodes, &g);
    }

    #[test]
    fn deterministic(nodes in arb_graph()) {
        let a = layout_graph_nodes(nodes.iter().map(|(c, ps)| (c, ps.as_slice())));
        let b = layout_graph_nodes(nodes.iter().map(|(c, ps)| (c, ps.as_slice())));
        prop_assert_eq!(a, b);
    }
}
