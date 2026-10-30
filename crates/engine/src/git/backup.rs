//! Index backup before stage and unstage (`impl-docs/git/stage.md`).
//!
//! Stage and unstage rewrite the staged version of a file. If that version is neither in
//! HEAD nor in the worktree (`git status` XY code with X ∉ {`.`, `?`} and Y ≠ `.`: `MM`, `AM`,
//! `AD`, `MD`, `RM`, ...) after the write it would survive only as an unreachable blob, which
//! git's garbage collector sooner or later deletes. Before writing, the backends save those
//! versions in a commit reachable from a ref under [`BACKUP_PREFIX`]; if the backup
//! fails the index is left untouched.

use project_domain::{Change, EntryKind, Oid, RepoPath, Status};
use std::collections::BTreeSet;

/// Prefix of the backup refs: `refs/project/backup/index-<date>`.
pub const BACKUP_PREFIX: &str = "refs/project/backup/";

/// A staged version to save before rewriting the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedVersion {
    pub path: RepoPath,
    /// Mode in the index (e.g. `0o100644`).
    pub mode: u32,
    /// Blob id in the index.
    pub oid: Oid,
}

/// True if `path` is `wanted` or a file under the folder `wanted`, like a literal git
/// pathspec.
pub fn path_matches(path: &RepoPath, wanted: &RepoPath) -> bool {
    let (path, wanted) = (path.as_bytes(), wanted.as_bytes());
    path == wanted || (path.starts_with(wanted) && path.get(wanted.len()) == Some(&b'/'))
}

/// The staged versions of the files touched by `paths` (also inside a requested folder) that
/// exist only in the index: different from HEAD (X ∉ {`.`, `?`}) and from the worktree (Y ≠ `.`). They are
/// the ones a stage or unstage would lose. Conflicted files do not count: unstage
/// rejects them and stage resolves them leaving their versions in the index's REUC
/// extension (`git checkout -m` restores them).
pub fn versions_at_risk(status: &Status, paths: &[RepoPath]) -> Vec<StagedVersion> {
    versions_at_risk_hidden(status, paths, &BTreeSet::new())
}

/// Like [`versions_at_risk`], but for the files in `hidden` the worktree is not reliable: with
/// `--assume-unchanged` or `--skip-worktree` `git status` gives Y = `.` even if the file is
/// different. For them it is enough that the staged version differs from HEAD (X ∉ {`.`, `?`}).
pub fn versions_at_risk_hidden(
    status: &Status,
    paths: &[RepoPath],
    hidden: &BTreeSet<RepoPath>,
) -> Vec<StagedVersion> {
    status
        .entries
        .iter()
        .filter_map(|entry| {
            let info = match &entry.kind {
                EntryKind::Ordinary(info)
                | EntryKind::Renamed { info, .. }
                | EntryKind::Copied { info, .. } => info,
                _ => return None,
            };
            let touched = paths.iter().any(|p| {
                path_matches(&entry.path, p)
                    || matches!(&entry.kind, EntryKind::Renamed { original, .. } if path_matches(original, p))
            });
            let at_risk = entry.index.is_change()
                && (entry.worktree != Change::Unmodified || hidden.contains(&entry.path))
                && info.mode_index != 0;
            (touched && at_risk).then(|| StagedVersion {
                path: entry.path.clone(),
                mode: info.mode_index,
                oid: info.oid_index,
            })
        })
        .collect()
}

/// Backup commit message: the operation and the saved paths.
pub fn backup_message(operation: &str, versions: &[StagedVersion]) -> String {
    let mut message = format!(
        "Backup of the index before {operation}\n\nStaged versions that were neither in HEAD \
         nor in the worktree:\n\n"
    );
    for v in versions {
        message.push_str(&format!("{}\n", v.path));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitRead, MemoryBackend};

    #[test]
    fn only_staged_versions_missing_from_head_and_worktree() {
        let status = MemoryBackend::sample().status().unwrap();
        let p = |s: &str| RepoPath::from(s);
        // README.md is `MM`: at risk. docs/guide.md `A.`, src/lib.rs `.M`, note.txt `??`: not.
        let all = [p("README.md"), p("docs"), p("src"), p("note.txt")];
        let risky: Vec<_> = versions_at_risk(&status, &all)
            .into_iter()
            .map(|v| v.path)
            .collect();
        assert_eq!(risky, [p("README.md")]);
        assert!(versions_at_risk(&status, &[p("docs"), p("src/lib.rs")]).is_empty());
        assert!(versions_at_risk(&status, &[p("READ")]).is_empty());
        // docs/guide.md is `A.`: at risk only if the worktree is not reliable.
        let hidden = BTreeSet::from([p("docs/guide.md")]);
        let risky: Vec<_> = versions_at_risk_hidden(&status, &all, &hidden)
            .into_iter()
            .map(|v| v.path)
            .collect();
        assert_eq!(risky, [p("README.md"), p("docs/guide.md")]);
    }

    #[test]
    fn matches_files_and_folders() {
        let p = |s: &str| RepoPath::from(s);
        assert!(path_matches(&p("a/b.txt"), &p("a")));
        assert!(path_matches(&p("a"), &p("a")));
        assert!(!path_matches(&p("ab/c"), &p("a")));
        assert!(!path_matches(&p("a"), &p("a/b")));
    }
}
