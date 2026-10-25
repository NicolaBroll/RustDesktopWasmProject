//! What changed inside a line: a removed line and the added line that takes its place are
//! compared word by word, and the parts that differ are highlighted on both sides.
//!
//! The pairs come from the hunk: a run of removed lines followed by a run of added lines, the
//! first with the first, the second with the second. Lines too different (less than a third in
//! common) are not highlighted: everything would be.

use project_domain::LineKind;
use std::ops::Range;

/// Char ranges of a line.
pub(crate) type Ranges = Vec<Range<usize>>;

/// Tokens compared with the full LCS at most (per line); beyond, only the common start and end.
const MAX_TOKENS: usize = 400;

/// The pairs `(removed, added)` of line indices in a hunk with these `kinds`.
pub(crate) fn pairs(kinds: &[LineKind]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < kinds.len() {
        if kinds[i] != LineKind::Removed {
            i += 1;
            continue;
        }
        let removed = i;
        while i < kinds.len() && kinds[i] == LineKind::Removed {
            i += 1;
        }
        let added = i;
        while i < kinds.len() && kinds[i] == LineKind::Added {
            i += 1;
        }
        let n = (added - removed).min(i - added);
        out.extend((0..n).map(|k| (removed + k, added + k)));
    }
    out
}

/// The changed parts of `old` and `new`, as char ranges; `None` if the lines have too little in
/// common to be worth highlighting.
pub(crate) fn changed(old: &str, new: &str) -> Option<(Ranges, Ranges)> {
    let a = tokens(old);
    let b = tokens(new);
    let common = if a.len() <= MAX_TOKENS && b.len() <= MAX_TOKENS {
        lcs(&a, &b)
    } else {
        ends(&a, &b)
    };
    let kept: usize = common.iter().map(|&(i, _)| a[i].len()).sum();
    let longest = old.chars().count().max(new.chars().count());
    if longest == 0 || kept * 3 < longest {
        return None;
    }
    let old_ranges = outside(&a, common.iter().map(|&(i, _)| i));
    let new_ranges = outside(&b, common.iter().map(|&(_, j)| j));
    if old_ranges.is_empty() && new_ranges.is_empty() {
        return None;
    }
    Some((old_ranges, new_ranges))
}

/// A token: a run of letters/digits/`_`, a run of spaces, or one other character. Each with its
/// first char and its length in chars.
#[derive(Debug, Clone, Copy)]
struct Token<'a> {
    text: &'a str,
    start: usize,
    chars: usize,
}

impl Token<'_> {
    fn len(&self) -> usize {
        self.chars
    }
}

fn tokens(line: &str) -> Vec<Token<'_>> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out: Vec<Token<'_>> = Vec::new();
    let mut last: Option<Class> = None;
    for (chars, (byte, c)) in line.char_indices().enumerate() {
        let kind = class(c);
        let joins = kind != Class::Other && last.as_ref() == Some(&kind);
        match out.last_mut() {
            Some(token) if joins => {
                token.text = &line[byte - token.text.len()..byte + c.len_utf8()];
                token.chars += 1;
            }
            _ => out.push(Token {
                text: &line[byte..byte + c.len_utf8()],
                start: chars,
                chars: 1,
            }),
        }
        last = Some(kind);
    }
    out
}

/// The pairs of equal tokens of a longest common subsequence.
fn lcs(a: &[Token<'_>], b: &[Token<'_>]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut table = vec![0u16; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[at(i, j)] = if a[i].text == b[j].text {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i].text == b[j].text {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if table[at(i + 1, j)] >= table[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// The equal tokens at the start and at the end (for lines too long for the LCS).
fn ends(a: &[Token<'_>], b: &[Token<'_>]) -> Vec<(usize, usize)> {
    let start = a
        .iter()
        .zip(b)
        .take_while(|(x, y)| x.text == y.text)
        .count();
    let end = a[start..]
        .iter()
        .rev()
        .zip(b[start..].iter().rev())
        .take_while(|(x, y)| x.text == y.text)
        .count();
    let mut out: Vec<(usize, usize)> = (0..start).map(|k| (k, k)).collect();
    out.extend((0..end).rev().map(|k| (a.len() - 1 - k, b.len() - 1 - k)));
    out
}

/// The char ranges of the tokens not in `kept`, adjacent ones merged.
fn outside(tokens: &[Token<'_>], kept: impl Iterator<Item = usize>) -> Vec<Range<usize>> {
    let mut keep = vec![false; tokens.len()];
    for k in kept {
        keep[k] = true;
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    for (token, kept) in tokens.iter().zip(keep) {
        if kept {
            continue;
        }
        let range = token.start..token.start + token.len();
        match out.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => out.push(range),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use LineKind::{Added as A, Context as C, Removed as R};

    #[test]
    fn removed_runs_pair_with_the_added_runs_after_them() {
        assert_eq!(pairs(&[C, R, A, C]), [(1, 2)]);
        assert_eq!(pairs(&[R, R, A, A, A]), [(0, 2), (1, 3)]);
        assert_eq!(pairs(&[R, C, A]), []);
        assert_eq!(pairs(&[A, R]), []);
        assert_eq!(pairs(&[R, A, C, R, R, A]), [(0, 1), (3, 5)]);
    }

    fn texts(line: &str, ranges: &[Range<usize>]) -> Vec<String> {
        let chars: Vec<char> = line.chars().collect();
        ranges
            .iter()
            .map(|r| chars[r.clone()].iter().collect())
            .collect()
    }

    #[test]
    fn only_the_changed_words_are_marked() {
        let (old, new) = ("let total = price * 2;", "let total = price * qty;");
        let (a, b) = changed(old, new).unwrap();
        assert_eq!(texts(old, &a), ["2"]);
        assert_eq!(texts(new, &b), ["qty"]);
    }

    #[test]
    fn an_insertion_marks_only_the_new_side() {
        let (old, new) = ("fn f(a: i32)", "fn f(a: i32, b: i32)");
        let (a, b) = changed(old, new).unwrap();
        assert!(a.is_empty());
        assert_eq!(texts(new, &b), [", b: i32"]);
    }

    #[test]
    fn lines_with_little_in_common_are_not_marked() {
        assert_eq!(changed("use std::fmt;", "    return Err(error);"), None);
        assert_eq!(changed("same", "same"), None);
        assert_eq!(changed("", ""), None);
    }

    #[test]
    fn non_ascii_ranges_are_in_chars() {
        let (old, new) = ("naïve = 1", "naïve = 2");
        let (a, b) = changed(old, new).unwrap();
        // `ï` is two bytes: the digit is char 8, byte 9.
        assert_eq!((a[0].clone(), b[0].clone()), (8..9, 8..9));
        assert_eq!((a.len(), b.len()), (1, 1));
    }

    #[test]
    fn long_lines_use_the_common_ends() {
        let old = format!("{} x {}", "a ".repeat(300), "b ".repeat(300));
        let new = format!("{} y {}", "a ".repeat(300), "b ".repeat(300));
        let (a, b) = changed(&old, &new).unwrap();
        assert_eq!(texts(&old, &a), ["x"]);
        assert_eq!(texts(&new, &b), ["y"]);
    }
}
