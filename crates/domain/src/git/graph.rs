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
    layout_graph_nodes(commits.iter().map(|c| (&c.id, c.parents.as_slice())))
}

/// Like [`layout_graph`], but it receives only the identifier and parents of each commit.
pub fn layout_graph_nodes<'a, I>(nodes: I) -> GraphLayout
where
    I: IntoIterator<Item = (&'a Oid, &'a [Oid])>,
{
    // Active lanes: `Some(oid)` waits for that commit, `None` is free.
    let mut lanes: Vec<Option<Oid>> = Vec::new();
    // Segments of the previous row not yet resolved: (from, lane). The arrival column
    // depends on where the next commit falls.
    let mut pending: Vec<(usize, usize)> = Vec::new();
    let mut rows: Vec<GraphRow> = Vec::new();

    for (id, parents) in nodes {
        // Step 2: the commit's column and the converging lanes.
        let waiting: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, w)| w.as_ref() == Some(id))
            .map(|(j, _)| j)
            .collect();
        let column = match waiting.first() {
            Some(&j) => j,
            None => first_free(&mut lanes),
        };

        // Now the arrival of the previous row's segments is known.
        if let Some(prev) = rows.last_mut() {
            prev.segments = resolve(&pending, &waiting, column);
        }
        for &j in &waiting {
            lanes[j] = None;
        }

        // Step 3: parents, without duplicates (there should be none, but an edge must be drawn
        // only once).
        pending.clear();
        // Vertical segments of the lanes passing next to the commit: those already open before
        // its parents (its lane has just become free).
        for (j, w) in lanes.iter().enumerate() {
            if w.is_some() {
                pending.push((j, j));
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
                    None => first_free(&mut lanes),
                }
            };
            lanes[lane] = Some(*parent);
            pending.push((column, lane));
        }

        // Free lanes at the end are no longer needed.
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }

        rows.push(GraphRow {
            column,
            segments: Vec::new(),
        });
    }

    // Last row: the segments stay dangling in their lane.
    if let Some(last) = rows.last_mut() {
        last.segments = resolve(&pending, &[], usize::MAX);
    }

    let width = rows
        .iter()
        .flat_map(|r| {
            std::iter::once(r.column).chain(r.segments.iter().flat_map(|s| [s.from, s.to]))
        })
        .max()
        .map_or(0, |m| m + 1);

    GraphLayout { rows, width }
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
