//! Blame and history of one file in the [`MemoryBackend`] (S34).
//!
//! The blame walks the chain of first parents version by version: the lines equal to the
//! parent's version (longest common subsequence, [`line_diff::matching_lines`]) go further back,
//! the others belong to the commit being looked at. The renames are those recorded in the
//! changes of each commit. The history compares the file with the first parent, like the
//! changes of each commit; a merge is shown only if the file differs from every parent.

use super::write::file_type;
use super::{MemoryBackend, line_diff};
use crate::git::{
    BlameQuery, FileBlame, FileLogQuery, GitRead, LogQuery, blame_binary, blame_missing_at,
    blame_missing_worktree, blame_no_commits, is_binary_content,
};
use crate::{EngineError, JobContext};
use project_domain::{
    Blame, BlameCommit, BlameLine, ByteText, Change, EntryKind, FileLogEntry, Oid, RepoPath,
};
use std::collections::BTreeMap;

const GITLINK: u32 = 0o160000;

/// Who a line belongs to: the commit and the file's name there (`None` = not committed), and
/// the line's index in that version.
type Owner = (Option<(Oid, RepoPath)>, usize);

/// A version of the file the blame is looking at: in a commit, or in the worktree (`None`).
struct Version {
    commit: Option<Oid>,
    path: RepoPath,
    content: Vec<u8>,
}

impl MemoryBackend {
    /// Content of file `path` in the tree of commit `id`; `None` if missing, a submodule or of
    /// unknown content.
    fn file_in(&self, id: Oid, path: &RepoPath) -> Option<Vec<u8>> {
        let (mode, oid) = *self.tree_at(id).get(path)?;
        if file_type(mode) == GITLINK {
            return None;
        }
        self.blobs.get(&oid).cloned()
    }

    /// The content of the tracked file `path` in the worktree and its name in HEAD.
    fn worktree_version(&self, path: &RepoPath) -> Result<(Vec<u8>, RepoPath), EngineError> {
        let missing = || blame_missing_worktree(path);
        let head = self.head_target().ok_or_else(blame_no_commits)?;
        let in_head = self.tree_at(head).get(path).copied();
        let Some(entry) = self.entries.iter().find(|e| e.path == *path) else {
            // Unchanged file: the HEAD version, unless the worktree was given.
            let (_, oid) = in_head.ok_or_else(missing)?;
            let content = self.worktree.get(path).or_else(|| self.blobs.get(&oid));
            return Ok((content.cloned().unwrap_or_default(), path.clone()));
        };
        let (info, head_path) = match &entry.kind {
            EntryKind::Ordinary(info) => (info, path.clone()),
            EntryKind::Renamed { info, original, .. }
            | EntryKind::Copied { info, original, .. } => (info, original.clone()),
            EntryKind::Unmerged(_) => {
                let content = self.worktree.get(path).ok_or_else(missing)?;
                return Ok((content.clone(), path.clone()));
            }
            EntryKind::Untracked | EntryKind::Ignored => return Err(missing()),
        };
        if entry.worktree == Change::Deleted {
            return Err(missing());
        }
        let content = match self.worktree.get(path) {
            Some(content) => content.clone(),
            None => self
                .blobs
                .get(&info.oid_index)
                .cloned()
                .ok_or_else(missing)?,
        };
        Ok((content, head_path))
    }

    /// The version `version` comes from: in the first parent (with the name the commit's rename
    /// gives it), or in HEAD for the worktree. `None` if the file did not exist there.
    fn previous_version(&self, version: &Version) -> Option<Version> {
        let (parent, path) = match version.commit {
            None => (self.head_target()?, version.path.clone()),
            Some(id) => {
                let parent = *self.commits.get(&id)?.parents.first()?;
                let changes = self.changes.get(&id).into_iter().flatten();
                let path = match changes.into_iter().find(|f| f.path == version.path) {
                    Some(f) if f.change == Change::Added => return None,
                    Some(f) => f.old_path().clone(),
                    None => version.path.clone(),
                };
                (parent, path)
            }
        };
        let content = self.file_in(parent, &path)?;
        Some(Version {
            commit: Some(parent),
            path,
            content,
        })
    }

    pub(super) fn memory_blame(
        &self,
        query: &BlameQuery,
        job: &JobContext,
    ) -> Result<FileBlame, EngineError> {
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "file.blame",
            });
        }
        let path = &query.path;
        let (rev, mut version) = match &query.rev {
            Some(rev) => {
                let id = self.resolve(rev)?;
                if !self.commits.contains_key(&id) {
                    return Err(EngineError::UnknownRev(rev.clone()));
                }
                let content = self
                    .file_in(id, path)
                    .ok_or_else(|| blame_missing_at(path, &id))?;
                let version = Version {
                    commit: Some(id),
                    path: path.clone(),
                    content,
                };
                (Some(id), version)
            }
            None => {
                let (content, head_path) = self.worktree_version(path)?;
                let version = Version {
                    commit: None,
                    path: head_path,
                    content,
                };
                (None, version)
            }
        };
        if is_binary_content(&version.content) {
            return Err(blame_binary(path));
        }
        let content = version.content.clone();
        let texts = split_lines(&content);
        // (line of the final file, line in `version`), still without a commit.
        let mut pending: Vec<(usize, usize)> = (0..texts.len()).map(|i| (i, i)).collect();
        // For each line: (commit key, line in that commit).
        let mut owner: Vec<Option<Owner>> = vec![None; texts.len()];
        let mut found: BTreeMap<(Oid, RepoPath), BlameCommit> = BTreeMap::new();
        while !pending.is_empty() {
            let previous = self.previous_version(&version);
            let matches = previous
                .as_ref()
                .map(|p| line_diff::matching_lines(&p.content, &version.content));
            let mut next = Vec::new();
            for (line, at) in pending {
                match matches.as_ref().and_then(|m| m[at]) {
                    Some(older) => next.push((line, older)),
                    None => {
                        let key = version.commit.map(|id| (id, version.path.clone()));
                        if let Some(key) = &key
                            && !found.contains_key(key)
                        {
                            found.insert(key.clone(), self.blame_commit(key, previous.as_ref()));
                        }
                        owner[line] = Some((key, at + 1));
                    }
                }
            }
            pending = next;
            match previous {
                Some(p) => version = p,
                None => break,
            }
        }
        // The commits in order of first appearance, like `--porcelain`.
        let mut blame = Blame::default();
        let mut index: BTreeMap<(Oid, RepoPath), usize> = BTreeMap::new();
        for (text, owner) in texts.into_iter().zip(owner) {
            let (key, orig) = owner.unwrap_or((None, 1));
            let commit = key.map(|key| {
                *index.entry(key.clone()).or_insert_with(|| {
                    blame.commits.push(found[&key].clone());
                    blame.commits.len() - 1
                })
            });
            blame.lines.push(BlameLine {
                text: ByteText::from(text),
                commit,
                orig_line: u32::try_from(orig).unwrap_or(u32::MAX),
            });
        }
        Ok(FileBlame {
            blame,
            path: path.clone(),
            rev,
        })
    }

    fn blame_commit(
        &self,
        (id, path): &(Oid, RepoPath),
        previous: Option<&Version>,
    ) -> BlameCommit {
        let commit = &self.commits[id];
        BlameCommit {
            id: *id,
            author: commit.author.clone(),
            committer: commit.committer.clone(),
            summary: commit.summary.clone(),
            path: path.clone(),
            previous: previous.and_then(|p| Some((p.commit?, p.path.clone()))),
            boundary: commit.parents.is_empty(),
        }
    }

    pub(super) fn memory_file_log(
        &self,
        query: &FileLogQuery,
        job: &JobContext,
    ) -> Result<Vec<FileLogEntry>, EngineError> {
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "file.log",
            });
        }
        let start = match &query.rev {
            Some(rev) => self.resolve(rev)?,
            None => match self.head_target() {
                Some(id) => id,
                None => return Ok(Vec::new()),
            },
        };
        let commits = self.log(&LogQuery::all().from_rev(start.to_string()))?;
        let mut path = query.path.clone();
        let mut out = Vec::new();
        for commit in commits {
            let changes = self.changes.get(&commit.id).map(Vec::as_slice);
            let changes = changes.unwrap_or_default();
            let entry = if let Some(f) = changes.iter().find(|f| f.path == path) {
                // A merge only if the file differs from every parent.
                let same_as_other = commit
                    .parents
                    .iter()
                    .skip(1)
                    .any(|p| self.tree_at(*p).get(&path) == Some(&(f.new_mode, f.new_oid)));
                if same_as_other {
                    continue;
                }
                match (f.change, f.original.clone()) {
                    (Change::Renamed | Change::Copied, Some(original)) if query.follow => {
                        let entry = (f.change, path.clone(), Some(original.clone()));
                        path = original;
                        entry
                    }
                    (Change::Renamed | Change::Copied, _) => (Change::Added, path.clone(), None),
                    (change, _) => (change, path.clone(), None),
                }
            } else if changes
                .iter()
                .any(|f| f.change == Change::Renamed && f.original.as_ref() == Some(&path))
            {
                (Change::Deleted, path.clone(), None)
            } else {
                continue;
            };
            let (status, path, old_path) = entry;
            out.push(FileLogEntry {
                commit,
                path,
                old_path,
                status,
            });
        }
        let limit = query.limit.unwrap_or(usize::MAX);
        Ok(out.into_iter().skip(query.skip).take(limit).collect())
    }
}

/// The lines of `content` without their newline.
fn split_lines(content: &[u8]) -> Vec<&[u8]> {
    let body = content.strip_suffix(b"\n").unwrap_or(content);
    if content.is_empty() {
        return Vec::new();
    }
    body.split(|&b| b == b'\n').collect()
}
