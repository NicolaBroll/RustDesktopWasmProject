//! The refs of the left column as a tree of rows: the names split on `/` become folders
//! (`feature/login` under `feature`, `origin/main` under the remote `origin`), with a filter.
//! The rows are flat and all the same height, so the column draws only those in view.

use project_domain::RefInfo;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

/// A row of a section.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Row {
    Folder {
        depth: usize,
        /// The folder's path from the section root (`origin/feature`): its id.
        path: String,
        name: String,
        /// How many refs it holds, sub-folders included.
        count: usize,
        open: bool,
    },
    Ref {
        depth: usize,
        /// Index in the refs given to [`rows`].
        index: usize,
        /// Where the last component starts in the ref's short name.
        leaf: usize,
    },
}

#[cfg(test)]
impl Row {
    fn depth(&self) -> usize {
        match self {
            Row::Folder { depth, .. } | Row::Ref { depth, .. } => *depth,
        }
    }
}

#[derive(Default)]
struct Node {
    folders: BTreeMap<String, Node>,
    refs: Vec<(String, usize, usize)>,
    count: usize,
}

/// The rows of `refs` (one section, sorted by name). `roots`: folders shown at the top level even
/// when empty (the remotes without branches). `filter` (already lowercase) keeps the refs whose
/// short name contains it and opens every folder; without it the folders in `closed` stay closed.
/// Folders come before the refs, both sorted by name; with `newest_first` (the tags) by version,
/// highest first: `v1.10` before `v1.9`.
pub(crate) fn rows(
    refs: &[&RefInfo],
    roots: &[&str],
    filter: &str,
    closed: &HashSet<String>,
    newest_first: bool,
) -> Vec<Row> {
    let mut root = Node::default();
    if filter.is_empty() {
        for name in roots {
            root.folders.entry((*name).to_owned()).or_default();
        }
    }
    for (index, info) in refs.iter().enumerate() {
        let name = info.short_name();
        if !filter.is_empty() && !name.to_lowercase().contains(filter) {
            continue;
        }
        let mut node = &mut root;
        node.count += 1;
        let mut start = 0;
        let parts: Vec<&str> = name.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            node = node.folders.entry((*part).to_owned()).or_default();
            node.count += 1;
            start += part.len() + 1;
        }
        node.refs.push((name[start..].to_owned(), index, start));
    }
    let order = |a: &str, b: &str| {
        if newest_first {
            natural_cmp(b, a)
        } else {
            a.cmp(b)
        }
    };
    let mut out = Vec::new();
    flatten(&root, 0, "", !filter.is_empty(), closed, &order, &mut out);
    out
}

/// Compares names with the runs of digits as numbers: `v1.9` < `v1.10`. Same numbers with different
/// zeros (`v01`, `v1`) fall back to the plain order, so the order is total.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.as_bytes(), b.as_bytes());
    while let (Some(&c), Some(&d)) = (x.first(), y.first()) {
        if c.is_ascii_digit() && d.is_ascii_digit() {
            let digits = |s: &[u8]| s.iter().take_while(|b| b.is_ascii_digit()).count();
            let (m, n) = (digits(x), digits(y));
            let trim = |s: &[u8]| {
                let zeros = s.iter().take_while(|&&b| b == b'0').count();
                s[zeros..].to_vec()
            };
            let (p, q) = (trim(&x[..m]), trim(&y[..n]));
            let by_value = p.len().cmp(&q.len()).then_with(|| p.cmp(&q));
            if by_value != Ordering::Equal {
                return by_value;
            }
            (x, y) = (&x[m..], &y[n..]);
        } else if c != d {
            return c.cmp(&d);
        } else {
            (x, y) = (&x[1..], &y[1..]);
        }
    }
    x.len().cmp(&y.len()).then_with(|| a.cmp(b))
}

fn flatten(
    node: &Node,
    depth: usize,
    path: &str,
    filtering: bool,
    closed: &HashSet<String>,
    order: &dyn Fn(&str, &str) -> Ordering,
    out: &mut Vec<Row>,
) {
    let mut folders: Vec<_> = node.folders.iter().collect();
    folders.sort_by(|a, b| order(a.0, b.0));
    for (name, child) in folders {
        let path = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}/{name}")
        };
        let open = filtering || !closed.contains(&path);
        out.push(Row::Folder {
            depth,
            path: path.clone(),
            name: name.clone(),
            count: child.count,
            open,
        });
        if open {
            flatten(child, depth + 1, &path, filtering, closed, order, out);
        }
    }
    let mut refs: Vec<_> = node.refs.iter().collect();
    refs.sort_by(|a, b| order(&a.0, &b.0));
    for (_, index, leaf) in refs {
        out.push(Row::Ref {
            depth,
            index: *index,
            leaf: *leaf,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Oid, RefKind};

    fn branch(name: &str) -> RefInfo {
        RefInfo {
            name: format!("refs/heads/{name}"),
            kind: RefKind::LocalBranch,
            target: Oid::from_hex("a".repeat(40).as_bytes()).unwrap(),
            tag_object: None,
            upstream: None,
        }
    }

    /// The rows as text: `+folder (n)` / `-folder (n)` (closed), refs by their leaf.
    fn show(refs: &[RefInfo], roots: &[&str], filter: &str, closed: &[&str]) -> Vec<String> {
        let list: Vec<&RefInfo> = refs.iter().collect();
        let closed = closed.iter().map(|c| (*c).to_owned()).collect();
        rows(&list, roots, filter, &closed, false)
            .into_iter()
            .map(|row| {
                let indent = "  ".repeat(row.depth());
                match row {
                    Row::Folder {
                        name, count, open, ..
                    } => format!("{indent}{}{name} ({count})", if open { '+' } else { '-' }),
                    Row::Ref { index, leaf, .. } => {
                        format!("{indent}{}", &refs[index].short_name()[leaf..])
                    }
                }
            })
            .collect()
    }

    #[test]
    fn slashes_become_folders_before_the_refs() {
        let refs = [
            branch("feature/a"),
            branch("feature/ui/b"),
            branch("main"),
            branch("fix/c"),
        ];
        assert_eq!(
            show(&refs, &[], "", &[]),
            [
                "+feature (2)",
                "  +ui (1)",
                "    b",
                "  a",
                "+fix (1)",
                "  c",
                "main"
            ]
        );
    }

    #[test]
    fn a_closed_folder_hides_its_content() {
        let refs = [branch("feature/a"), branch("feature/ui/b"), branch("main")];
        assert_eq!(show(&refs, &[], "", &["feature"]), ["-feature (2)", "main"]);
        assert_eq!(
            show(&refs, &[], "", &["feature/ui"]),
            ["+feature (2)", "  -ui (1)", "  a", "main"]
        );
    }

    #[test]
    fn the_filter_keeps_the_matches_and_opens_the_folders() {
        let refs = [
            branch("feature/Login"),
            branch("feature/ui"),
            branch("main"),
        ];
        assert_eq!(
            show(&refs, &["origin"], "log", &["feature"]),
            ["+feature (1)", "  Login"]
        );
        assert!(show(&refs, &[], "nothing", &[]).is_empty());
    }

    #[test]
    fn newest_first_puts_the_highest_version_on_top() {
        let refs = [
            branch("v0.9.0"),
            branch("v0.10.0"),
            branch("v0.2.1"),
            branch("release/v1"),
            branch("release/v2"),
        ];
        let list: Vec<&RefInfo> = refs.iter().collect();
        let names: Vec<&str> = rows(&list, &[], "", &HashSet::new(), true)
            .into_iter()
            .map(|row| match row {
                Row::Folder { .. } => "release/",
                Row::Ref { index, .. } => refs[index].short_name(),
            })
            .collect();
        assert_eq!(
            names,
            [
                "release/",
                "release/v2",
                "release/v1",
                "v0.10.0",
                "v0.9.0",
                "v0.2.1"
            ]
        );
    }

    #[test]
    fn natural_order_compares_numbers_by_value() {
        assert_eq!(natural_cmp("v1.9", "v1.10"), Ordering::Less);
        assert_eq!(natural_cmp("v2", "v10"), Ordering::Less);
        assert_eq!(natural_cmp("a", "b"), Ordering::Less);
        assert_eq!(natural_cmp("v1", "v1.0"), Ordering::Less);
        assert_ne!(natural_cmp("v01", "v1"), Ordering::Equal);
    }

    #[test]
    fn roots_show_even_when_empty() {
        let refs = [branch("origin/main")];
        assert_eq!(
            show(&refs, &["origin", "backup"], "", &[]),
            ["+backup (0)", "+origin (1)", "  main"]
        );
    }
}
