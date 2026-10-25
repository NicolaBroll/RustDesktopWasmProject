//! Line diff for [`super::MemoryBackend`]: the hunks `git diff` would write with the
//! default settings (3 lines of context, nearby hunks merged).
//!
//! It uses the longest common subsequence, in `O(n·m)` time and memory: enough for the small files
//! of an in-memory repo, not for real files.

use project_domain::{ByteText, Hunk, Line, LineKind};

/// Context lines around each group of changes, like git's default.
const CONTEXT: usize = 3;

/// A line of the file: the text without the newline, and whether the newline was there. Two lines are equal only if
/// both match, as in git (`x` at the end of a file without newline differs from `x\n`).
#[derive(PartialEq, Eq)]
struct Source<'a> {
    text: &'a [u8],
    newline: bool,
}

fn split(content: &[u8]) -> Vec<Source<'_>> {
    let mut lines = Vec::new();
    let mut rest = content;
    while !rest.is_empty() {
        match rest.iter().position(|&b| b == b'\n') {
            Some(i) => {
                lines.push(Source {
                    text: &rest[..i],
                    newline: true,
                });
                rest = &rest[i + 1..];
            }
            None => {
                lines.push(Source {
                    text: rest,
                    newline: false,
                });
                rest = &[];
            }
        }
    }
    lines
}

#[derive(Clone, Copy)]
enum Op {
    /// Equal line: index in the old and in the new.
    Equal(usize, usize),
    /// Removed line: index in the old.
    Delete(usize),
    /// Added line: index in the new.
    Insert(usize),
}

/// The operations leading from `a` to `b`; in a changed block the removed lines come before
/// the added ones, as in git.
fn ops(a: &[Source], b: &[Source]) -> Vec<Op> {
    let (n, m) = (a.len(), b.len());
    // lcs[i][j]: length of the longest common subsequence of a[i..] and b[j..].
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(Op::Equal(i, j));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(Op::Delete(i));
            i += 1;
        } else {
            out.push(Op::Insert(j));
            j += 1;
        }
    }
    out.extend((i..n).map(Op::Delete));
    out.extend((j..m).map(Op::Insert));
    out
}

fn number(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// The hunks of the diff from `old` to `new` (whole contents of the two files). Empty if they are equal.
pub(super) fn hunks(old: &[u8], new: &[u8]) -> Vec<Hunk> {
    let (a, b) = (split(old), split(new));
    let ops = ops(&a, &b);

    // Groups of changes (indices in `ops`, ends included): two changes separated by at
    // most 2 × CONTEXT equal lines end up in the same hunk.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        if matches!(op, Op::Equal(..)) {
            continue;
        }
        match groups.last_mut() {
            Some((_, end)) if i - *end <= 2 * CONTEXT + 1 => *end = i,
            _ => groups.push((i, i)),
        }
    }

    // Lines of the old and of the new before each operation.
    let mut before = Vec::with_capacity(ops.len() + 1);
    let (mut old_seen, mut new_seen) = (0, 0);
    for op in &ops {
        before.push((old_seen, new_seen));
        match op {
            Op::Equal(..) => {
                old_seen += 1;
                new_seen += 1;
            }
            Op::Delete(_) => old_seen += 1,
            Op::Insert(_) => new_seen += 1,
        }
    }

    groups
        .into_iter()
        .map(|(start, end)| {
            let from = start.saturating_sub(CONTEXT);
            let to = (end + CONTEXT).min(ops.len() - 1);
            let (old_before, new_before) = before[from];
            let (mut old_count, mut new_count) = (0, 0);
            let lines: Vec<Line> = ops[from..=to]
                .iter()
                .map(|op| {
                    let (kind, source, old_line, new_line) = match *op {
                        Op::Equal(i, j) => (
                            LineKind::Context,
                            &a[i],
                            Some(number(i + 1)),
                            Some(number(j + 1)),
                        ),
                        Op::Delete(i) => (LineKind::Removed, &a[i], Some(number(i + 1)), None),
                        Op::Insert(j) => (LineKind::Added, &b[j], None, Some(number(j + 1))),
                    };
                    old_count += usize::from(old_line.is_some());
                    new_count += usize::from(new_line.is_some());
                    Line {
                        kind,
                        old_line,
                        new_line,
                        content: ByteText::from_bytes(source.text),
                        no_newline: !source.newline,
                    }
                })
                .collect();
            // With zero lines the start is the line after which it inserts (0 = start of the file).
            let start_line = |seen: usize, count: usize| number(seen + usize::from(count > 0));
            Hunk {
                old_start: start_line(old_before, old_count),
                old_count: number(old_count),
                new_start: start_line(new_before, new_count),
                new_count: number(new_count),
                section: ByteText::default(),
                lines,
            }
        })
        .collect()
}

/// For each line of `new`, the index of the equal line of `old` it comes from (longest common
/// subsequence), `None` for a line added or changed: the matching blame needs.
pub(super) fn matching_lines(old: &[u8], new: &[u8]) -> Vec<Option<usize>> {
    let (a, b) = (split(old), split(new));
    let mut out = vec![None; b.len()];
    for op in ops(&a, &b) {
        if let Op::Equal(i, j) = op {
            out[j] = Some(i);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hunks written the way git writes them.
    fn text(old: &str, new: &str) -> String {
        let mut out = Vec::new();
        for hunk in hunks(old.as_bytes(), new.as_bytes()) {
            hunk.write_to(&mut out);
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn equal_gives_no_hunk() {
        assert_eq!(text("a\nb\n", "a\nb\n"), "");
        assert_eq!(text("", ""), "");
    }

    #[test]
    fn new_and_deleted_file() {
        assert_eq!(text("", "a\nb\n"), "@@ -0,0 +1,2 @@\n+a\n+b\n");
        assert_eq!(text("a\n", ""), "@@ -1 +0,0 @@\n-a\n");
    }

    #[test]
    fn change_with_context() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let new = "1\n2\n3\n4\nFIVE\n6\n7\n8\n";
        assert_eq!(
            text(old, new),
            "@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-5\n+FIVE\n 6\n 7\n 8\n"
        );
    }

    #[test]
    fn far_hunks_stay_separate_near_ones_merge() {
        let file = |changed: &[usize]| -> String {
            (1..=20)
                .map(|n| match changed.contains(&n) {
                    true => format!("line {n} changed\n"),
                    false => format!("{n}\n"),
                })
                .collect()
        };
        let old = file(&[]);
        // Between the two changes 7 equal lines: two hunks.
        let far = text(&old, &file(&[2, 10]));
        assert_eq!(far.matches("@@ -").count(), 2, "{far}");
        assert!(far.starts_with("@@ -1,5 +1,5 @@\n"), "{far}");
        assert!(far.contains("@@ -7,7 +7,7 @@\n"), "{far}");
        // With 6 equal lines: a single hunk.
        let near = text(&old, &file(&[2, 9]));
        assert!(near.starts_with("@@ -1,12 +1,12 @@\n"), "{near}");
        assert_eq!(near.matches("@@ -").count(), 1, "{near}");
    }

    #[test]
    fn last_line_without_newline() {
        assert_eq!(
            text("a\nb", "a\nb\n"),
            "@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n"
        );
        let h = hunks(b"x", b"y");
        assert!(h[0].lines.iter().all(|l| l.no_newline));
    }

    #[test]
    fn insertion_in_the_middle() {
        assert_eq!(
            text("a\nb\n", "a\nnew\nb\n"),
            "@@ -1,2 +1,3 @@\n a\n+new\n b\n"
        );
    }
}
