//! Commit graph lanes (`impl-docs/ARCHITECTURE-git.md` §3).
//!
//! Pure function: it receives the commits in topological order, most recent first (like `git log`), and
//! returns one row per commit with the commit's column and the segments to draw up to
//! the next row.
//!
//! Algorithm:
//! 1. keep a vector of active lanes; each lane "waits" for an [`Oid`];
//! 2. for each commit the column is the first lane waiting for it, or else the first free lane;
//!    the other lanes that were waiting for it converge here and become free;
//! 3. the commit's lane goes on to wait for the first parent; every other parent goes on the
//!    first lane already waiting for it, or else on the first free lane (a fork opens).
//!
//! Lanes do not move: a lane stays in the same column as long as it is open, so the
//! column can also serve as the color index. Free lanes are reused and the free
//! ones at the end are dropped, so the width stays bounded.
//!
//! Non-topological or truncated input (e.g. `git log -n 100`) causes no errors: a parent that does not
//! appear later keeps its lane open until the last row, whose segments stay
//! "dangling" downwards.

use serde::{Deserialize, Serialize};

use super::{Commit, Oid};

/// A parent–child edge segment between one row and the next.
///
/// It starts from the row it is in, at column `from`, and arrives at the next row at
/// column `to`. If `from` is the column of the row's commit, the segment leaves that commit;
/// if `to` is the column of the next row's commit, the segment enters that commit;
/// otherwise the segment continues in the next row from `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphSegment {
    /// Starting column, in the segment's row.
    pub from: usize,
    /// Arrival column, in the next row.
    pub to: usize,
    /// Lane the edge travels on (useful for the color): it equals `to` for a
    /// fork, `from` for a convergence, and both for a vertical segment.
    pub lane: usize,
}

/// A graph row: a commit and the segments towards the next row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphRow {
    /// Column of the commit.
    pub column: usize,
    /// Segments towards the next row, sorted by `(lane, from)`. In the last row they are the
    /// segments towards parents that do not appear in the input.
    pub segments: Vec<GraphSegment>,
}

/// Layout of the graph: one row per commit, in the same order as the input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphLayout {
    pub rows: Vec<GraphRow>,
    /// Number of columns needed: `1 +` the largest column used by a commit or segment
    /// (`0` if there are no commits).
    pub width: usize,
}

/// Computes the lanes for a list of commits in topological order, most recent first.
pub fn layout_graph(commits: &[Commit]) -> GraphLayout {
    let mut builder = GraphBuilder::default();
    builder.extend_commits(commits);
    builder.into_layout()
}

/// Like [`layout_graph`], but it receives only the identifier and parents of each commit.
pub fn layout_graph_nodes<'a, I>(nodes: I) -> GraphLayout
where
    I: IntoIterator<Item = (&'a Oid, &'a [Oid])>,
{
    let mut builder = GraphBuilder::default();
    builder.extend(nodes);
    builder.into_layout()
}

/// Computes a [`GraphLayout`] a page at a time: every [`GraphBuilder::extend`] adds the rows of
/// the commits that follow, at the cost of those commits only. The result is the same as
/// [`layout_graph`] on all the commits together.
///
/// Rows already there do not change, except the segments of the last one: they were dangling
/// and now arrive at the first commit added.
#[derive(Debug, Clone, Default)]
pub struct GraphBuilder {
    /// Active lanes: `Some(oid)` waits for that commit, `None` is free.
    lanes: Vec<Option<Oid>>,
    /// Segments of the last row not yet resolved: (from, lane). The arrival column
    /// depends on where the next commit falls.
    pending: Vec<(usize, usize)>,
    layout: GraphLayout,
    /// Width of the rows whose segments are final: all but the last.
    settled: usize,
}

impl GraphBuilder {
    /// The layout of the commits received so far.
    pub fn layout(&self) -> &GraphLayout {
        &self.layout
    }

    pub fn into_layout(self) -> GraphLayout {
        self.layout
    }

    /// Adds the rows of `commits`, which follow the ones already received.
    pub fn extend_commits(&mut self, commits: &[Commit]) {
        self.extend(commits.iter().map(|c| (&c.id, c.parents.as_slice())));
    }

    /// Like [`GraphBuilder::extend_commits`], with only the identifier and parents of each commit.
    pub fn extend<'a, I>(&mut self, nodes: I)
    where
        I: IntoIterator<Item = (&'a Oid, &'a [Oid])>,
    {
        for (id, parents) in nodes {
            self.push(id, parents);
        }
        // Last row: the segments stay dangling in their lane.
        self.layout.width = match self.layout.rows.last_mut() {
            Some(last) => {
                last.segments = resolve(&self.pending, &[], usize::MAX);
                self.settled.max(row_width(last))
            }
            None => 0,
        };
    }

    fn push(&mut self, id: &Oid, parents: &[Oid]) {
        let lanes = &mut self.lanes;
        // Step 2: the commit's column and the converging lanes.
        let waiting: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, w)| w.as_ref() == Some(id))
            .map(|(j, _)| j)
            .collect();
        let column = match waiting.first() {
            Some(&j) => j,
            None => first_free(lanes),
        };

        // Now the arrival of the previous row's segments is known.
        if let Some(prev) = self.layout.rows.last_mut() {
            prev.segments = resolve(&self.pending, &waiting, column);
            self.settled = self.settled.max(row_width(prev));
        }
        for &j in &waiting {
            lanes[j] = None;
        }

        // Step 3: parents, without duplicates (there should be none, but an edge must be drawn
        // only once).
        self.pending.clear();
        // Vertical segments of the lanes passing next to the commit: those already open before
        // its parents (its lane has just become free).
        for (j, w) in lanes.iter().enumerate() {
            if w.is_some() {
                self.pending.push((j, j));
            }
        }
        let mut seen: Vec<&Oid> = Vec::with_capacity(parents.len());
        for parent in parents {
            if seen.contains(&parent) {
                continue;
            }
            seen.push(parent);
            let lane = if seen.len() == 1 {
                column
            } else {
                match lanes.iter().position(|w| w.as_ref() == Some(parent)) {
                    Some(j) => j,
                    None => first_free(lanes),
                }
            };
            lanes[lane] = Some(*parent);
            self.pending.push((column, lane));
        }

        // Free lanes at the end are no longer needed.
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }

        self.layout.rows.push(GraphRow {
            column,
            segments: Vec::new(),
        });
    }
}

/// Columns a row needs: `1 +` the largest column of its commit and segments.
fn row_width(row: &GraphRow) -> usize {
    row.segments
        .iter()
        .flat_map(|s| [s.from, s.to])
        .fold(row.column, usize::max)
        + 1
}

/// First free lane, adding one at the end if needed.
fn first_free(lanes: &mut Vec<Option<Oid>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(j) => j,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

/// Turns the pending segments into segments: one travelling on a lane that was waiting for the
/// next commit converges on its column, the others go straight on.
fn resolve(pending: &[(usize, usize)], waiting: &[usize], column: usize) -> Vec<GraphSegment> {
    let mut segments: Vec<GraphSegment> = pending
        .iter()
        .map(|&(from, lane)| GraphSegment {
            from,
            to: if waiting.contains(&lane) {
                column
            } else {
                lane
            },
            lane,
        })
        .collect();
    segments.sort_by_key(|s| (s.lane, s.from));
    segments
}
