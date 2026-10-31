//! `worktree.ignore` and its undo/redo on the [`MemoryBackend`].
//!
//! The root `.gitignore` is the worktree file `.gitignore` (or, if the worktree map does not
//! have it, the index version of a tracked `.gitignore`); `.git/info/exclude` is a field. The
//! status hides the untracked files that the two files ignore, with a simplified matcher
//! ([`matches_rules`]: `*`, `**`, `?`, `\` escapes, `!`, anchoring and trailing `/`; no
//! `[...]` classes, no `.gitignore` in subfolders, no `core.excludesFile`). The backup is a
//! fake ref `refs/project/backup/ignore-<n>`; the blobs go in the blob store.

use super::MemoryBackend;
use super::write::{entry_order, tracked_info};
use crate::git::{BACKUP_PREFIX, IgnoreFile, IgnoreOutcome, IgnoreRecord, IgnoreRequest};
use crate::{EngineError, IgnoreRestore, RestoreOutcome};
use project_domain::{
    Change, EntryKind, IgnorePattern, RepoPath, StatusEntry, TrackedInfo, append_ignore_patterns,
};

const GITIGNORE: &str = ".gitignore";

impl MemoryBackend {
    /// The content of the ignore file `file` (`None`: it does not exist).
    pub fn ignore_file(&self, file: IgnoreFile) -> Option<Vec<u8>> {
        match file {
            IgnoreFile::Exclude => self.exclude.clone(),
            IgnoreFile::Gitignore => {
                let path = RepoPath::from(GITIGNORE);
                if let Some(content) = self.worktree.get(&path) {
                    return Some(content.clone());
                }
                let deleted = self
                    .entries
                    .iter()
                    .any(|e| e.path == path && e.worktree == Change::Deleted);
                let (_, oid) = self.index_tree().get(&path).copied()?;
                if deleted {
                    return None;
                }
                Some(self.blobs.get(&oid).cloned().unwrap_or_default())
            }
        }
    }

    /// Writes the ignore file `file` (`None` removes it) as another program would, and updates
    /// the status entry of `.gitignore` (untracked, or modified relative to the index).
    pub fn set_ignore_file(&mut self, file: IgnoreFile, content: Option<Vec<u8>>) {
        if file == IgnoreFile::Exclude {
            self.exclude = content;
            return;
        }
        let path = RepoPath::from(GITIGNORE);
        let Some((mode, oid)) = self.index_tree().get(&path).copied() else {
            match content {
                Some(bytes) => self.add_untracked(path, bytes),
                None => self.remove_worktree_file(&path),
            }
            return;
        };
        let Some(bytes) = content else {
            self.worktree.remove(&path);
            return;
        };
        let same = self.blobs.get(&oid) == Some(&bytes);
        self.worktree.insert(path.clone(), bytes);
        let existing = self
            .entries
            .iter()
            .position(|e| e.path == path && tracked_info(e).is_some());
        let (mut entry, mut info) = match existing {
            Some(i) => {
                let entry = self.entries.remove(i);
                let info = tracked_info(&entry).cloned().expect("tracked entry");
                (entry, info)
            }
            None => (
                StatusEntry {
                    path: path.clone(),
                    index: Change::Unmodified,
                    worktree: Change::Unmodified,
                    kind: EntryKind::Ordinary(TrackedInfo {
                        submodule: None,
                        mode_head: mode,
                        mode_index: mode,
                        mode_worktree: mode,
                        oid_head: oid,
                        oid_index: oid,
                    }),
                },
                TrackedInfo {
                    submodule: None,
                    mode_head: mode,
                    mode_index: mode,
                    mode_worktree: mode,
                    oid_head: oid,
                    oid_index: oid,
                },
            ),
        };
        info.mode_worktree = info.mode_index;
        entry.worktree = if same {
            Change::Unmodified
        } else {
            Change::Modified
        };
        self.entries.extend(self.finish(entry, info));
        self.entries
            .sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
    }

    /// True if the untracked file `path` is ignored by the root `.gitignore` or by
    /// `.git/info/exclude` (simplified matcher, see the module).
    pub(super) fn is_ignored(&self, path: &RepoPath) -> bool {
        let mut rules = Vec::new();
        for file in [IgnoreFile::Exclude, IgnoreFile::Gitignore] {
            if let Some(content) = self.ignore_file(file) {
                rules.extend(content.split(|&b| b == b'\n').map(<[u8]>::to_vec));
            }
        }
        matches_rules(&rules, path.as_bytes())
    }

    /// The untracked entries, as the status shows them (without the ignored ones).
    fn visible_untracked(&self) -> Vec<RepoPath> {
        self.entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Untracked) && !self.is_ignored(&e.path))
            .map(|e| e.path.clone())
            .collect()
    }

    /// [`GitIndex::ignore`](crate::GitIndex::ignore). An error changes nothing.
    pub(super) fn ignore_patterns(
        &mut self,
        request: &IgnoreRequest,
    ) -> Result<IgnoreOutcome, EngineError> {
        let file = request.file;
        if file == IgnoreFile::Gitignore
            && self.entries.iter().any(|e| {
                e.path.as_bytes() == GITIGNORE.as_bytes()
                    && matches!(e.kind, EntryKind::Unmerged(_))
            })
        {
            return Err(EngineError::NotAllowed(msg!("engine-ignore-conflicted")));
        }
        let old = self.ignore_file(file);
        let append = append_ignore_patterns(old.as_deref(), &request.patterns);
        let mut outcome = IgnoreOutcome {
            file,
            added: append.added,
            already: append.already,
            ignored: Vec::new(),
            tracked: Vec::new(),
            created: false,
            record: None,
            backup: None,
            warnings: Vec::new(),
        };
        if outcome.added.is_empty() {
            return Ok(outcome);
        }
        let before_untracked = self.visible_untracked();
        let before = old.clone().map(|bytes| self.store_blob(bytes));
        let after = self.store_blob(append.content.clone());
        let n = self.ignore_backups + 1;
        self.ignore_backups = n;
        self.set_ignore_file(file, Some(append.content));
        let after_untracked = self.visible_untracked();
        outcome.ignored = before_untracked
            .into_iter()
            .filter(|p| !after_untracked.contains(p))
            .collect();
        outcome.tracked = self.tracked_matching(&outcome.added);
        outcome.created = old.is_none();
        outcome.record = Some(IgnoreRecord {
            file,
            before,
            after,
        });
        outcome.backup = Some(format!("{BACKUP_PREFIX}ignore-{n}"));
        Ok(outcome)
    }

    /// The tracked files (index) that `patterns` alone match, in path order.
    fn tracked_matching(&self, patterns: &[IgnorePattern]) -> Vec<RepoPath> {
        let rules: Vec<Vec<u8>> = patterns.iter().map(|p| p.as_bytes().to_vec()).collect();
        self.index_tree()
            .into_keys()
            .filter(|path| matches_rules(&rules, path.as_bytes()))
            .collect()
    }

    /// [`GitIndex::restore_ignore`](crate::GitIndex::restore_ignore). An error changes
    /// nothing.
    pub(super) fn restore_ignore_file(
        &mut self,
        request: &IgnoreRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        let (expected, wanted) = request.blobs();
        let content = |oid: Option<project_domain::Oid>| -> Result<Option<Vec<u8>>, EngineError> {
            oid.map(|oid| {
                self.blobs
                    .get(&oid)
                    .cloned()
                    .ok_or_else(|| EngineError::GitWithReason {
                        command: "restore".to_owned(),
                        reason: msg!("engine-memory-backup-missing", stash = oid),
                    })
            })
            .transpose()
        };
        let expected = content(expected)?;
        let wanted = content(wanted)?;
        if self.ignore_file(request.record.file) != expected {
            return Err(request.conflict());
        }
        self.set_ignore_file(request.record.file, wanted);
        Ok(RestoreOutcome::default())
    }
}

/// True if the lines `rules` of a root ignore file ignore `path` (a file): the last rule that
/// matches the path or one of its folders decides; a folder ignored ignores everything under
/// it.
pub(super) fn matches_rules(rules: &[Vec<u8>], path: &[u8]) -> bool {
    let parsed: Vec<Rule> = rules.iter().filter_map(|l| Rule::parse(l)).collect();
    let ends: Vec<usize> = path
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'/')
        .map(|(i, _)| i)
        .chain([path.len()])
        .collect();
    for (k, &end) in ends.iter().enumerate() {
        let prefix = &path[..end];
        let is_dir = end < path.len();
        let name_start = if k == 0 { 0 } else { ends[k - 1] + 1 };
        let name = &path[name_start..end];
        let decision = parsed
            .iter()
            .rev()
            .find(|r| {
                (!r.dir_only || is_dir)
                    && if r.anchored {
                        glob(&r.pattern, prefix)
                    } else {
                        glob(&r.pattern, name)
                    }
            })
            .map(|r| !r.negated);
        if decision == Some(true) {
            return true;
        }
    }
    false
}

struct Rule {
    pattern: Vec<u8>,
    negated: bool,
    anchored: bool,
    dir_only: bool,
}

impl Rule {
    fn parse(line: &[u8]) -> Option<Rule> {
        let mut line = line.strip_suffix(b"\r").unwrap_or(line).to_vec();
        // Trailing spaces, unless escaped.
        while line.ends_with(b" ") && !line.ends_with(b"\\ ") {
            line.pop();
        }
        if line.is_empty() || line[0] == b'#' {
            return None;
        }
        let negated = line[0] == b'!';
        if negated {
            line.remove(0);
        }
        let dir_only = line.ends_with(b"/");
        if dir_only {
            line.pop();
        }
        let anchored = line.contains(&b'/');
        if line.first() == Some(&b'/') {
            line.remove(0);
        }
        (!line.is_empty()).then_some(Rule {
            pattern: line,
            negated,
            anchored,
            dir_only,
        })
    }
}

/// Wildcard match of `text` against `pattern`: `*` (not across `/`), `**` (anything), `?`,
/// `\x` (a literal `x`).
fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.first() {
        None => text.is_empty(),
        Some(b'*') if pattern.get(1) == Some(&b'*') => {
            let rest = pattern[2..].strip_prefix(b"/").unwrap_or(&pattern[2..]);
            (0..=text.len()).any(|i| glob(rest, &text[i..]))
        }
        Some(b'*') => {
            let rest = &pattern[1..];
            for i in 0..=text.len() {
                if glob(rest, &text[i..]) {
                    return true;
                }
                if text.get(i) == Some(&b'/') {
                    break;
                }
            }
            false
        }
        Some(b'?') => text.first().is_some_and(|&c| c != b'/') && glob(&pattern[1..], &text[1..]),
        Some(b'\\') if pattern.len() > 1 => {
            text.first() == Some(&pattern[1]) && glob(&pattern[2..], &text[1..])
        }
        Some(&c) => text.first() == Some(&c) && glob(&pattern[1..], &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::matches_rules;
    use crate::{
        EngineError, GitIndex, GitRead, IgnoreFile, IgnoreRequest, IgnoreRestore, MemoryBackend,
    };
    use project_domain::{EntryKind, IgnoreKind, IgnorePattern, RepoPath, ignore_pattern};

    fn rules(lines: &[&str]) -> Vec<Vec<u8>> {
        lines.iter().map(|l| l.as_bytes().to_vec()).collect()
    }

    #[test]
    fn simplified_matcher() {
        let r = rules(&["# c", "*.log", "!keep.log", "/build/", "/a/b", "x\\*"]);
        for (path, ignored) in [
            ("d/e.log", true),
            ("keep.log", false),
            ("build/x/y", true),
            ("src/build/y", false),
            ("a/b", true),
            ("a/b/c", true),
            ("z/a/b", false),
            ("x*", true),
            ("xy", false),
        ] {
            assert_eq!(matches_rules(&r, path.as_bytes()), ignored, "{path}");
        }
        // The patterns of `ignore_pattern` match what they say.
        let p = |path: &str, kind| {
            vec![
                ignore_pattern(&RepoPath::from(path), kind)
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            ]
        };
        assert!(matches_rules(&p("d/a b ", IgnoreKind::Path), b"d/a b "));
        assert!(!matches_rules(&p("d/a b ", IgnoreKind::Path), b"d/a b"));
        assert!(matches_rules(&p("d/#!*", IgnoreKind::Path), b"d/#!*"));
        assert!(!matches_rules(&p("d/#!*", IgnoreKind::Path), b"d/#!x"));
        assert!(matches_rules(&p("d", IgnoreKind::Folder), b"d/x"));
        assert!(matches_rules(
            &p("q.log", IgnoreKind::Extension),
            b"z/w.log"
        ));
    }

    fn untracked(repo: &MemoryBackend) -> Vec<String> {
        repo.status()
            .unwrap()
            .entries
            .into_iter()
            .filter(|e| matches!(e.kind, EntryKind::Untracked))
            .map(|e| e.path.to_string())
            .collect()
    }

    #[test]
    fn ignore_undo_redo_and_conflicts() {
        let mut repo = MemoryBackend::sample();
        repo.add_untracked("out.log", "x");
        let before = repo.ignore_file(IgnoreFile::Gitignore).unwrap();
        let request = IgnoreRequest {
            file: IgnoreFile::Gitignore,
            patterns: vec![IgnorePattern::from("*.log")],
        };
        let out = repo.ignore(&request).unwrap();
        assert_eq!(out.ignored, [RepoPath::from("out.log")]);
        assert!(!out.created);
        let record = out.record.unwrap();
        let mut expected = before.clone();
        expected.extend_from_slice(b"*.log\n");
        assert_eq!(repo.ignore_file(IgnoreFile::Gitignore).unwrap(), expected);
        // `.gitignore` is tracked: it shows up modified.
        let status = repo.status().unwrap();
        assert!(
            status
                .entries
                .iter()
                .any(|e| e.path == RepoPath::from(".gitignore")
                    && e.worktree == project_domain::Change::Modified)
        );

        let mut undo = IgnoreRestore {
            command: "edit.undo",
            redo: false,
            record,
        };
        // Changed in the meantime: refused, nothing changes.
        repo.set_ignore_file(IgnoreFile::Gitignore, Some(b"other\n".to_vec()));
        assert!(matches!(
            repo.restore_ignore(&undo),
            Err(EngineError::UndoConflict { .. })
        ));
        assert_eq!(repo.ignore_file(IgnoreFile::Gitignore).unwrap(), b"other\n");
        repo.set_ignore_file(IgnoreFile::Gitignore, Some(expected.clone()));
        repo.restore_ignore(&undo).unwrap();
        assert_eq!(repo.ignore_file(IgnoreFile::Gitignore).unwrap(), before);
        assert!(untracked(&repo).contains(&"out.log".to_owned()));
        undo.redo = true;
        undo.command = "edit.redo";
        repo.restore_ignore(&undo).unwrap();
        assert_eq!(repo.ignore_file(IgnoreFile::Gitignore).unwrap(), expected);
    }
}
