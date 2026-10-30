//! Drawing of the commit graph: lane colors, edge geometry and ref labels. The lanes are
//! computed by `project_domain::layout_graph`; here is only how to show them, in pure
//! functions that can be tested without opening a window.

use egui::{Color32, Pos2, pos2};
use project_domain::{GraphLayout, GraphSegment, Head, Oid, RefInfo, RefKind};
use std::collections::HashMap;

/// Width of a lane, in points.
pub const LANE_WIDTH: f32 = 14.0;
/// Radius of a commit dot.
pub const NODE_RADIUS: f32 = 4.5;
/// Thickness of the edges.
pub const EDGE_WIDTH: f32 = 2.0;

/// Lane colors: saturated enough to read on light and dark backgrounds, neighbors different
/// from each other. They repeat after the last one.
const PALETTE: [Color32; 8] = [
    Color32::from_rgb(0x3d, 0x9e, 0xf5), // blue
    Color32::from_rgb(0xf5, 0x8b, 0x2d), // orange
    Color32::from_rgb(0x3c, 0xba, 0x6a), // green
    Color32::from_rgb(0xe0, 0x4f, 0x8f), // pink
    Color32::from_rgb(0x9b, 0x6b, 0xdf), // purple
    Color32::from_rgb(0xd9, 0xb8, 0x2b), // yellow
    Color32::from_rgb(0x2b, 0xb8, 0xb8), // turquoise
    Color32::from_rgb(0xe0, 0x55, 0x45), // red
];

/// The color of lane `lane` (stable as long as the lane stays open).
pub fn lane_color(lane: usize) -> Color32 {
    PALETTE[lane % PALETTE.len()]
}

/// The color of a branch name: the same name always gets the same color, whatever lane it is
/// on (FNV-1a hash of the name, stable across runs).
pub fn name_color(name: &str) -> Color32 {
    let hash = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    PALETTE[(hash % PALETTE.len() as u64) as usize]
}

/// The branch name a label gives to its lane: local branches as they are, remote branches without
/// `<remote>/` (one of `remotes`), so `main` and `origin/main` share a color. Tags, stashes and
/// a detached HEAD name no lane.
fn lane_name<'a>(label: &'a RefLabel, remotes: &[String]) -> Option<&'a str> {
    match label.kind {
        LabelKind::CurrentBranch | LabelKind::Branch => Some(&label.text),
        LabelKind::Remote => Some(
            remotes
                .iter()
                .find_map(|remote| {
                    label
                        .text
                        .strip_prefix(remote.as_str())
                        .and_then(|rest| rest.strip_prefix('/'))
                })
                .unwrap_or(&label.text),
        ),
        LabelKind::Head | LabelKind::Tag | LabelKind::Stash => None,
    }
}

/// The colors of the graph, row by row: the dot of each commit and each of its segments (in the
/// order of [`project_domain::GraphRow::segments`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LaneColors {
    pub commits: Vec<Color32>,
    pub segments: Vec<Vec<Color32>>,
}

impl LaneColors {
    /// The color of the commit of row `row` (the first lane's for a row out of range).
    pub fn commit(&self, row: usize) -> Color32 {
        self.commits.get(row).copied().unwrap_or(PALETTE[0])
    }

    /// The color of segment `segment` of row `row`.
    pub fn segment(&self, row: usize, segment: usize) -> Color32 {
        self.segments
            .get(row)
            .and_then(|s| s.get(segment))
            .copied()
            .unwrap_or(PALETTE[0])
    }
}

/// The colors of `layout` (rows of the commits `ids`, same order). A lane keeps one color from
/// where it opens to where it closes: that of the first branch found on it from the top (see
/// [`name_color`]), so adding a commit on another lane does not repaint it; a lane without a
/// branch takes the color of its column.
pub fn lane_colors(
    layout: &GraphLayout,
    ids: &[Oid],
    labels: &HashMap<Oid, Vec<RefLabel>>,
    remotes: &[String],
) -> LaneColors {
    // A run is one opening of a lane: its column and the name of the first branch on it.
    let mut runs: Vec<(usize, Option<&str>)> = Vec::new();
    let mut commit_runs = Vec::with_capacity(layout.rows.len());
    let mut segment_runs = Vec::with_capacity(layout.rows.len());
    // The run entering each lane from the row above.
    let mut incoming: HashMap<usize, usize> = HashMap::new();
    for (index, row) in layout.rows.iter().enumerate() {
        let column = row.column;
        let run = match incoming.get(&column) {
            Some(&run) => run,
            None => open_run(&mut runs, column),
        };
        if runs[run].1.is_none() {
            runs[run].1 = ids
                .get(index)
                .and_then(|id| labels.get(id))
                .and_then(|list| list.iter().find_map(|l| lane_name(l, remotes)));
        }
        commit_runs.push(run);
        let mut outgoing: HashMap<usize, usize> = HashMap::new();
        let mut row_runs = Vec::with_capacity(row.segments.len());
        for segment in &row.segments {
            let lane = segment.lane;
            let seg_run = if lane == column {
                run
            } else if let Some(&r) = outgoing.get(&lane) {
                r
            } else if row
                .segments
                .iter()
                .any(|s| s.from == lane && s.lane == lane)
            {
                // A lane passing by (also when a second parent joins it).
                match incoming.get(&lane) {
                    Some(&r) => r,
                    None => open_run(&mut runs, lane),
                }
            } else {
                // A fork: a lane opens for a parent.
                open_run(&mut runs, lane)
            };
            outgoing.insert(lane, seg_run);
            row_runs.push(seg_run);
        }
        segment_runs.push(row_runs);
        incoming = row
            .segments
            .iter()
            .filter(|s| s.to == s.lane)
            .map(|s| (s.lane, outgoing[&s.lane]))
            .collect();
    }
    let color = |run: usize| {
        let (lane, name) = runs[run];
        name.map_or_else(|| lane_color(lane), name_color)
    };
    LaneColors {
        commits: commit_runs.into_iter().map(color).collect(),
        segments: segment_runs
            .into_iter()
            .map(|row| row.into_iter().map(color).collect())
            .collect(),
    }
}

/// A new run of the lane `lane`, without a name yet.
fn open_run(runs: &mut Vec<(usize, Option<&str>)>, lane: usize) -> usize {
    runs.push((lane, None));
    runs.len() - 1
}

/// Width of the graph column for `lanes` lanes, with a margin on each side.
pub fn graph_width(lanes: usize) -> f32 {
    lanes.max(1) as f32 * LANE_WIDTH + LANE_WIDTH / 2.0
}

/// X of the center of column `column`, if the graph column starts at `left`.
pub fn column_x(left: f32, column: usize) -> f32 {
    left + LANE_WIDTH / 4.0 + (column as f32 + 0.5) * LANE_WIDTH
}

/// A stroke to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edge {
    /// Straight (vertical) stroke between two points.
    Line([Pos2; 2]),
    /// Cubic Bezier curve: moves the edge from one column to another.
    Curve([Pos2; 4]),
}

/// The stroke of `segment`: from the center of its row (y `y`) to the center of the next
/// row (`y + step`). If it changes column it is an S curve that starts and ends vertical.
pub fn segment_edge(segment: &GraphSegment, left: f32, y: f32, step: f32) -> Edge {
    let start = pos2(column_x(left, segment.from), y);
    let end = pos2(column_x(left, segment.to), y + step);
    if segment.from == segment.to {
        Edge::Line([start, end])
    } else {
        let mid = y + step / 2.0;
        Edge::Curve([start, pos2(start.x, mid), pos2(end.x, mid), end])
    }
}

/// Kind of label, in the order they are shown on the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LabelKind {
    /// HEAD detached on this commit.
    Head,
    /// The branch HEAD is on.
    CurrentBranch,
    Branch,
    Remote,
    Tag,
    /// A stash, on the commit it was created on (its base).
    Stash,
}

/// A label to show on a commit's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub kind: LabelKind,
    pub text: String,
}

/// Labels per commit: local branches (the current one first), remotes, tags, and `HEAD` if
/// detached. `refs/stash` has no label: stashes are added with [`add_stash_labels`],
/// on their base.
pub fn ref_labels(refs: &[RefInfo], head: &Head) -> HashMap<Oid, Vec<RefLabel>> {
    let mut labels: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
    let current = match head {
        Head::Branch { name, .. } => Some(name.as_str()),
        Head::Detached(_) => None,
    };
    let mut current_found = false;
    for info in refs {
        let text = info.short_name().to_owned();
        let kind = match info.kind {
            RefKind::LocalBranch if current == Some(text.as_str()) => {
                current_found = true;
                LabelKind::CurrentBranch
            }
            RefKind::LocalBranch => LabelKind::Branch,
            RefKind::RemoteBranch => LabelKind::Remote,
            RefKind::Tag => LabelKind::Tag,
            RefKind::Stash => continue,
        };
        labels
            .entry(info.target)
            .or_default()
            .push(RefLabel { kind, text });
    }
    // HEAD also arrives before the refs (from `repo.open`): the current branch is shown right away.
    match head {
        Head::Detached(id) => labels.entry(*id).or_default().push(RefLabel {
            kind: LabelKind::Head,
            text: "HEAD".to_owned(),
        }),
        Head::Branch {
            name,
            target: Some(id),
        } if !current_found => labels.entry(*id).or_default().push(RefLabel {
            kind: LabelKind::CurrentBranch,
            text: name.clone(),
        }),
        Head::Branch { .. } => {}
    }
    for list in labels.values_mut() {
        list.sort_by(|a, b| (a.kind, &a.text).cmp(&(b.kind, &b.text)));
    }
    labels
}

/// Appends to the labels of each commit the stashes created on top of it: `(base, name)`,
/// from the most recent (like `stash.list`). Several stashes on the same base stay in the
/// order of the list.
pub fn add_stash_labels<'a>(
    labels: &mut HashMap<Oid, Vec<RefLabel>>,
    stashes: impl IntoIterator<Item = (Oid, &'a str)>,
) {
    for (base, name) in stashes {
        labels.entry(base).or_default().push(RefLabel {
            kind: LabelKind::Stash,
            text: name.to_owned(),
        });
    }
}

/// How many labels of widths `widths` (space between them included) fit in
/// `available`. If they do not all fit the `+N` indicator, of width `more`, is needed too: the
/// shown labels leave room for that as well.
pub fn labels_that_fit(widths: &[f32], available: f32, more: f32) -> usize {
    let total: f32 = widths.iter().sum();
    if total <= available {
        return widths.len();
    }
    let mut used = more;
    widths
        .iter()
        .take_while(|&&w| {
            used += w;
            used <= available
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::Upstream;

    fn oid(n: u8) -> Oid {
        Oid::from_bytes([n; 20])
    }

    fn info(name: &str, kind: RefKind, target: u8) -> RefInfo {
        RefInfo {
            name: name.to_owned(),
            kind,
            target: oid(target),
            tag_object: None,
            upstream: None::<Upstream>,
        }
    }

    /// The colors of the commits `(id, parents)` (most recent first) with `refs` as labels.
    fn commit_colors(nodes: &[(u8, &[u8])], refs: &[RefInfo]) -> HashMap<u8, Color32> {
        let ids: Vec<Oid> = nodes.iter().map(|&(id, _)| oid(id)).collect();
        let parents: Vec<Vec<Oid>> = nodes
            .iter()
            .map(|&(_, p)| p.iter().map(|&n| oid(n)).collect())
            .collect();
        let layout =
            project_domain::layout_graph_nodes(ids.iter().zip(parents.iter().map(Vec::as_slice)));
        let head = Head::Branch {
            name: "main".to_owned(),
            target: None,
        };
        let labels = ref_labels(refs, &head);
        let colors = lane_colors(&layout, &ids, &labels, &["origin".to_owned()]);
        nodes
            .iter()
            .enumerate()
            .map(|(row, &(id, _))| (id, colors.commit(row)))
            .collect()
    }

    #[test]
    fn a_branch_keeps_its_color_when_another_lane_moves() {
        let feature = || info("refs/heads/feature/docs", RefKind::LocalBranch, 3);
        let origin = || info("refs/remotes/origin/main", RefKind::RemoteBranch, 2);
        // main and origin/main on 2, feature/docs on 3, both from 1.
        let before = commit_colors(
            &[(2, &[1]), (3, &[1]), (1, &[])],
            &[
                info("refs/heads/main", RefKind::LocalBranch, 2),
                origin(),
                feature(),
            ],
        );
        // A commit on main: feature/docs is now the first lane and main the second.
        let after = commit_colors(
            &[(4, &[2]), (3, &[1]), (2, &[1]), (1, &[])],
            &[
                info("refs/heads/main", RefKind::LocalBranch, 4),
                origin(),
                feature(),
            ],
        );
        let (main, docs) = (name_color("main"), name_color("feature/docs"));
        assert_ne!(main, docs, "the test needs two different colors");
        assert_eq!((before[&2], before[&3]), (main, docs));
        assert_eq!((after[&4], after[&2], after[&3]), (main, main, docs));
        // A commit on feature/docs, now first in the list.
        let after = commit_colors(
            &[(5, &[3]), (3, &[1]), (2, &[1]), (1, &[])],
            &[
                info("refs/heads/main", RefKind::LocalBranch, 2),
                origin(),
                info("refs/heads/feature/docs", RefKind::LocalBranch, 5),
            ],
        );
        assert_eq!((after[&5], after[&3], after[&2]), (docs, docs, main));
    }

    #[test]
    fn a_remote_branch_shares_the_color_of_the_local_one() {
        let colors = commit_colors(
            &[(2, &[1]), (3, &[1]), (1, &[])],
            &[
                info("refs/remotes/origin/main", RefKind::RemoteBranch, 3),
                info("refs/heads/topic", RefKind::LocalBranch, 2),
            ],
        );
        assert_eq!(colors[&3], name_color("main"));
        assert_eq!(colors[&2], name_color("topic"));
    }

    #[test]
    fn adjacent_lanes_have_different_colors() {
        for lane in 0..PALETTE.len() * 2 {
            assert_ne!(lane_color(lane), lane_color(lane + 1), "lane {lane}");
        }
        assert_eq!(lane_color(1), lane_color(1 + PALETTE.len()));
    }

    #[test]
    fn columns_are_inside_the_graph() {
        let left = 100.0;
        for lanes in 1..6 {
            let right = left + graph_width(lanes);
            let first = column_x(left, 0);
            let last = column_x(left, lanes - 1);
            assert!(first - NODE_RADIUS > left, "{lanes} lanes");
            assert!(last + NODE_RADIUS < right, "{lanes} lanes");
            assert_eq!(column_x(left, 1) - first, LANE_WIDTH);
        }
        assert_eq!(graph_width(0), graph_width(1));
    }

    #[test]
    fn straight_and_curved_edges() {
        let vertical = GraphSegment {
            from: 1,
            to: 1,
            lane: 1,
        };
        let x1 = column_x(0.0, 1);
        assert_eq!(
            segment_edge(&vertical, 0.0, 10.0, 20.0),
            Edge::Line([pos2(x1, 10.0), pos2(x1, 30.0)])
        );

        // Branch from column 0 to 2: starts and ends vertical, curves in the middle.
        let branch = GraphSegment {
            from: 0,
            to: 2,
            lane: 2,
        };
        let Edge::Curve([a, b, c, d]) = segment_edge(&branch, 0.0, 10.0, 20.0) else {
            panic!("expected a curve");
        };
        let (x0, x2) = (column_x(0.0, 0), column_x(0.0, 2));
        assert_eq!((a, d), (pos2(x0, 10.0), pos2(x2, 30.0)));
        assert_eq!((b.x, c.x), (x0, x2));
        assert_eq!((b.y, c.y), (20.0, 20.0));
    }

    #[test]
    fn labels_per_commit_in_order() {
        let refs = vec![
            info("refs/tags/v1", RefKind::Tag, 1),
            info("refs/remotes/origin/main", RefKind::RemoteBranch, 1),
            info("refs/heads/main", RefKind::LocalBranch, 1),
            info("refs/heads/alfa", RefKind::LocalBranch, 1),
            info("refs/heads/feature", RefKind::LocalBranch, 2),
            info("refs/stash", RefKind::Stash, 3),
        ];
        let head = Head::Branch {
            name: "main".to_owned(),
            target: Some(oid(1)),
        };
        let labels = ref_labels(&refs, &head);
        let texts = |n: u8| -> Vec<(LabelKind, &str)> {
            labels[&oid(n)]
                .iter()
                .map(|l| (l.kind, l.text.as_str()))
                .collect()
        };
        assert_eq!(
            texts(1),
            [
                (LabelKind::CurrentBranch, "main"),
                (LabelKind::Branch, "alfa"),
                (LabelKind::Remote, "origin/main"),
                (LabelKind::Tag, "v1"),
            ]
        );
        assert_eq!(texts(2), [(LabelKind::Branch, "feature")]);
        assert!(!labels.contains_key(&oid(3)), "no label for refs/stash");
    }

    #[test]
    fn stash_labels_go_last_on_their_base() {
        let refs = vec![info("refs/heads/main", RefKind::LocalBranch, 1)];
        let head = Head::Branch {
            name: "main".to_owned(),
            target: Some(oid(1)),
        };
        let mut labels = ref_labels(&refs, &head);
        add_stash_labels(
            &mut labels,
            [
                (oid(1), "stash@{0}"),
                (oid(2), "stash@{1}"),
                (oid(1), "stash@{2}"),
            ],
        );
        let texts = |n: u8| -> Vec<(LabelKind, &str)> {
            labels[&oid(n)]
                .iter()
                .map(|l| (l.kind, l.text.as_str()))
                .collect()
        };
        assert_eq!(
            texts(1),
            [
                (LabelKind::CurrentBranch, "main"),
                (LabelKind::Stash, "stash@{0}"),
                (LabelKind::Stash, "stash@{2}"),
            ]
        );
        assert_eq!(texts(2), [(LabelKind::Stash, "stash@{1}")]);
    }

    #[test]
    fn head_labels_without_refs() {
        let labels = ref_labels(&[], &Head::Detached(oid(4)));
        assert_eq!(labels[&oid(4)][0].kind, LabelKind::Head);

        let head = Head::Branch {
            name: "main".to_owned(),
            target: Some(oid(5)),
        };
        let labels = ref_labels(&[], &head);
        assert_eq!(labels[&oid(5)][0].text, "main");

        let unborn = Head::Branch {
            name: "main".to_owned(),
            target: None,
        };
        assert!(ref_labels(&[], &unborn).is_empty());
    }

    #[test]
    fn labels_leave_room_for_the_counter() {
        assert_eq!(labels_that_fit(&[30.0, 30.0], 60.0, 20.0), 2);
        assert_eq!(labels_that_fit(&[30.0, 30.0, 30.0], 80.0, 20.0), 2);
        assert_eq!(labels_that_fit(&[30.0, 30.0, 30.0], 70.0, 20.0), 1);
        assert_eq!(labels_that_fit(&[50.0], 10.0, 20.0), 0);
        assert_eq!(labels_that_fit(&[], 10.0, 20.0), 0);
    }
}
