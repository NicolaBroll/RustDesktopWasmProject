//! In-memory fake repository: for command and UI tests.

mod branch;
mod clean;
mod clone;
mod commit;
mod conflict;
mod discard;
mod discard_lines;
mod file;
mod ignore;
mod merge;
mod pick;
mod rebase;
mod remote;
mod remote_admin;
mod reset;
mod restore;
mod search;
mod stash;
mod submodule;
mod sync;
mod tags;
mod write;

pub use clean::CleanStash;
pub use discard::{StashedFile, WorktreeStash};
use merge::MemoryMerge;
use rebase::MemoryRebase;
pub use remote::FetchFailure;
use remote::MemoryRemote;
use stash::MemoryStash;

use super::line_diff;
use super::{
    BlameQuery, BranchDeleted, CommitOutcome, CommitRequest, ConfigEntry, ConflictResolution,
    ConflictResolved, ConflictVersions, DiffSpec, DiscardOutcome, DiscardedFile, FetchOutcome,
    FetchRequest, FileBlame, FileLogQuery, GitBackend, GitIndex, GitMaintenance, GitOperations,
    GitRead, GitRefs, GitRemotes, GitStash, HeadState, LineSelection, LogQuery, MergeAborted,
    MergeOutcome, MergeRequest, OperationState, PickKind, PickRequest, PullOutcome, PullRequest,
    PullSettings, PushOutcome, PushRequest, RebaseRequest, RefRestore, RemoteBranchDelete,
    RemoteBranchDeleted, RemoteSnapshot, RemoteTagDelete, RepoOpener, RestoreOutcome,
    StagedVersion, StashApplied, StashApply, StashDrop, StashDropped, StashEntry, StashPush,
    TagPush, TagPushed, UpstreamConfig, WorktreeRestore,
};
use super::{CloneOutcome, CloneRequest, InitRequest, SearchQuery};
use crate::{CommitGraphState, EngineError, JobContext, Message, Progress};
use project_domain::{
    Change, ChangedFile, Commit, CommitDetails, Diff, EntryKind, FileDiff, FileLogEntry, Head, Oid,
    Operation, RefInfo, RefKind, RepoPath, Status, StatusEntry, Upstream,
};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::sync::Mutex;

const HEADS: &str = "refs/heads/";
const REMOTES: &str = "refs/remotes/";
const TAGS: &str = "refs/tags/";

/// Where HEAD points: the branch is held by name, so it follows the branch's commits.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HeadRef {
    /// Short name of the branch, even if it does not exist yet (empty repo).
    Branch(String),
    Detached(Oid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefEntry {
    target: Oid,
    tag_object: Option<Oid>,
}

impl RefEntry {
    /// The value of the ref as git stores it: for an annotated tag the tag object.
    fn value(&self) -> Oid {
        self.tag_object.unwrap_or(self.target)
    }
}

/// Repository built in code, without processes or file system.
///
/// Start from [`MemoryBackend::new`] (empty repo on `main`) or from [`MemoryBackend::sample`]
/// (sample repo) and add commits and refs with the `insert_commit`, `set_*` methods.
/// Ahead/behind of upstreams are computed from the commit graph at every read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryBackend {
    commits: BTreeMap<Oid, Commit>,
    /// Files changed by each commit relative to the first parent; a missing commit changes
    /// nothing.
    changes: BTreeMap<Oid, Vec<ChangedFile>>,
    /// Full name → target; the map order is that of `git for-each-ref`.
    refs: BTreeMap<String, RefEntry>,
    /// Local branch → remote branch, both full names.
    upstreams: BTreeMap<String, String>,
    head: HeadRef,
    entries: Vec<StatusEntry>,
    operation: Option<Operation>,
    /// Content of the blob objects, for diffs.
    blobs: BTreeMap<Oid, Vec<u8>>,
    /// Content of the files in the worktree, for `diff.worktree`.
    worktree: BTreeMap<RepoPath, Vec<u8>>,
    /// Index backups saved by stage and unstage: ref name and saved versions.
    backups: Vec<(String, Vec<StagedVersion>)>,
    /// Backup refs of commits (rewritten by `amend`, abandoned by undo/redo): name and commit.
    commit_backups: Vec<(String, Oid)>,
    /// Worktree backups saved by `worktree.discard`.
    worktree_backups: Vec<WorktreeStash>,
    /// Backups saved by `worktree.clean`.
    clean_backups: Vec<CleanStash>,
    /// Backups saved by `worktree.discard_lines`: ref name, path and blob with the old bytes.
    line_backups: Vec<(String, RepoPath, Oid)>,
    /// If set, the backup of `worktree.discard_lines` fails with this message
    /// ([`MemoryBackend::fail_line_backups`]).
    line_backup_failure: Option<String>,
    /// Name and email of the committer (`user.name`/`user.email`); `None` = not configured.
    identity: Option<(String, String)>,
    /// Configured remotes, by name.
    remotes: BTreeMap<String, MemoryRemote>,
    /// The pull configuration (`pull.rebase`, `pull.ff`), the same for all branches.
    pull_settings: PullSettings,
    /// If set, a `pre-push` hook that rejects every push with this message.
    pre_push_failure: Option<String>,
    /// Other configuration keys ([`GitRead::config_values`]).
    config: BTreeMap<String, Vec<String>>,
    /// The "servers" of removed remotes, by URL: a remote added with the same URL finds them again.
    detached_remotes: BTreeMap<String, MemoryRemote>,
    /// `remote.<name>.*` entries "outside the repo file" ([`MemoryBackend::set_remote_elsewhere`]).
    remote_elsewhere: BTreeMap<String, Vec<String>>,
    /// Remotes that `remotes()` lists even after removal
    /// ([`MemoryBackend::keep_remote_listed`]).
    listed_anyway: BTreeSet<String>,
    /// How many `restore_refs` calls still succeed before failing
    /// ([`MemoryBackend::fail_restore_refs_after`]); `None`: always.
    restore_budget: Option<usize>,
    /// If set, `rename_remote` fails: before writing (`false`) or after moving the configuration
    /// (`true`) ([`MemoryBackend::fail_remote_rename`]).
    remote_rename_failure: Option<bool>,
    /// The stashes, most recent first (`stash@{0}`); `refs/stash` points to the first.
    stashes: Vec<MemoryStash>,
    /// The dropped stashes, by id: `stash_store` (the undo of a drop) finds them again whole.
    dropped_stashes: BTreeMap<Oid, MemoryStash>,
    /// If set, every `stash_drop` fails with this message
    /// ([`MemoryBackend::fail_stash_drop`]).
    stash_drop_failure: Option<String>,
    /// If set, every write of the upstream configuration fails with this message
    /// ([`MemoryBackend::fail_config_writes`]).
    config_write_failure: Option<String>,
    /// The merge in progress (`MERGE_HEAD`, `MERGE_MSG`), with `operation` = `Merge`.
    merge_state: Option<MemoryMerge>,
    /// The rebase in progress, with `operation` = `Rebase`.
    rebase_state: Option<MemoryRebase>,
    /// The squash merge in progress, with `operation` = `Squash`.
    squash_state: Option<crate::git::SquashRecord>,
    /// There is a commit-graph ([`GitMaintenance::commit_graph`]): written by
    /// [`GitMaintenance::write_commit_graph`].
    commit_graph: bool,
    /// The annotated tag objects created by `tag.create`, with the commit they point to: an undo
    /// and redo of the tag put the ref back on the same object.
    tag_objects: BTreeMap<Oid, Oid>,
    /// The content of `.git/info/exclude` (`None`: no file).
    exclude: Option<Vec<u8>>,
    /// How many backups `worktree.ignore` saved (`refs/project/backup/ignore-<n>`).
    ignore_backups: u64,
    /// The submodules ([`MemoryBackend::set_submodules`]).
    submodules: Vec<super::SubmoduleInfo>,
    /// Backups saved by `submodule.update` inside the submodules: ref, path, HEAD saved.
    submodule_backups: Vec<(String, RepoPath, Oid)>,
}

impl Default for MemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryBackend {
    /// Empty repo: HEAD on `main`, which has no commits yet; clean worktree.
    pub fn new() -> Self {
        Self {
            commits: BTreeMap::new(),
            changes: BTreeMap::new(),
            refs: BTreeMap::new(),
            upstreams: BTreeMap::new(),
            head: HeadRef::Branch("main".to_owned()),
            entries: Vec::new(),
            operation: None,
            blobs: BTreeMap::new(),
            worktree: BTreeMap::new(),
            backups: Vec::new(),
            commit_backups: Vec::new(),
            worktree_backups: Vec::new(),
            clean_backups: Vec::new(),
            line_backups: Vec::new(),
            line_backup_failure: None,
            identity: None,
            remotes: BTreeMap::new(),
            pull_settings: PullSettings::default(),
            pre_push_failure: None,
            config: BTreeMap::new(),
            detached_remotes: BTreeMap::new(),
            remote_elsewhere: BTreeMap::new(),
            listed_anyway: BTreeSet::new(),
            restore_budget: None,
            remote_rename_failure: None,
            stashes: Vec::new(),
            dropped_stashes: BTreeMap::new(),
            stash_drop_failure: None,
            config_write_failure: None,
            merge_state: None,
            rebase_state: None,
            squash_state: None,
            commit_graph: false,
            tag_objects: BTreeMap::new(),
            exclude: None,
            ignore_backups: 0,
            submodules: Vec::new(),
            submodule_backups: Vec::new(),
        }
    }

    /// Fake, deterministic id: the same `seed` always gives the same id, different seeds different
    /// ids (in practice). The digits look random, like real ones.
    pub fn fake_oid(seed: u64) -> Oid {
        // splitmix64: mixed enough not to give similar ids for nearby seeds.
        let mut state = seed;
        let mut bytes = [0u8; Oid::LEN];
        for chunk in bytes.chunks_mut(8) {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            chunk.copy_from_slice(&z.to_be_bytes()[..chunk.len()]);
        }
        Oid::from_bytes(bytes)
    }

    /// Adds (or replaces) a commit. Parents missing from the repo are ignored by the log,
    /// as in a shallow clone.
    pub fn insert_commit(&mut self, commit: Commit) {
        self.commits.insert(commit.id, commit);
    }

    /// Files changed by commit `id` relative to the first parent (or to the empty tree for a
    /// root commit), in the order git would give them. Returned by
    /// [`GitRead::show_commit`]; without this call the commit changes no files.
    pub fn set_changes(&mut self, id: Oid, files: Vec<ChangedFile>) {
        self.changes.insert(id, files);
    }

    /// Content of blob `id`, used by the diffs of files that have it as id. A file whose
    /// content is not known shows up in diffs as binary (without hunks).
    pub fn set_blob(&mut self, id: Oid, content: impl Into<Vec<u8>>) {
        self.blobs.insert(id, content.into());
    }

    /// Content of file `path` in the worktree, for [`DiffSpec::Worktree`]: needed for the status
    /// entries modified outside the stage (not for deleted files).
    pub fn set_worktree_file(&mut self, path: impl Into<RepoPath>, content: impl Into<Vec<u8>>) {
        self.worktree.insert(path.into(), content.into());
    }

    /// The index backups saved by stage and unstage, in order: ref name and saved staged
    /// versions (the contents are in the blobs).
    pub fn backups(&self) -> &[(String, Vec<StagedVersion>)] {
        &self.backups
    }

    /// The backup refs of commits, in order: ref name and commit. Created by `amend`
    /// (`refs/project/backup/amend-<n>`, the rewritten commit) and `edit.undo`/`edit.redo`
    /// (`refs/project/backup/undo-<n>`/`redo-<n>`, the commits that would no longer be
    /// reachable).
    pub fn commit_backups(&self) -> &[(String, Oid)] {
        &self.commit_backups
    }

    /// Identity of the committer, like `user.name` and `user.email`. Without it (the default of
    /// [`MemoryBackend::new`]) a commit fails, like git without an identity.
    pub fn set_identity(&mut self, name: &str, email: &str) {
        self.identity = Some((name.to_owned(), email.to_owned()));
    }

    /// The content of blob `id`, if known.
    pub fn blob(&self, id: Oid) -> Option<&[u8]> {
        self.blobs.get(&id).map(Vec::as_slice)
    }

    /// Creates or moves the local branch `name` (short name, e.g. `main`).
    pub fn set_branch(&mut self, name: &str, target: Oid) {
        self.set_ref(format!("{HEADS}{name}"), target, None);
    }

    /// Creates or moves the remote branch `name` (short name, e.g. `origin/main`).
    pub fn set_remote_branch(&mut self, name: &str, target: Oid) {
        self.set_ref(format!("{REMOTES}{name}"), target, None);
    }

    /// Creates or moves the tag `name` on commit `target`. `tag_object` is the id of the tag object
    /// for an annotated tag, `None` for a lightweight one.
    pub fn set_tag(&mut self, name: &str, target: Oid, tag_object: Option<Oid>) {
        if let Some(object) = tag_object {
            self.tag_objects.insert(object, target);
        }
        self.set_ref(format!("{TAGS}{name}"), target, tag_object);
    }

    /// The local branch `branch` follows the remote branch `remote` (short names, e.g. `main` and
    /// `origin/main`). The remote may also not exist: the upstream shows up as gone.
    pub fn set_upstream(&mut self, branch: &str, remote: &str) {
        self.upstreams
            .insert(format!("{HEADS}{branch}"), format!("{REMOTES}{remote}"));
    }

    /// HEAD on branch `name` (short name), which may not exist yet.
    pub fn set_head_branch(&mut self, name: &str) {
        self.head = HeadRef::Branch(name.to_owned());
    }

    /// HEAD detached on commit `target`.
    pub fn detach_head(&mut self, target: Oid) {
        self.head = HeadRef::Detached(target);
    }

    /// Status entries, in the order git would give them.
    pub fn set_entries(&mut self, entries: Vec<StatusEntry>) {
        self.entries = entries;
    }

    /// Operation in progress (merge, rebase, ...), `None` if none. A merge set this way
    /// has no `MERGE_HEAD` (the commit rejects it); a real merge is left by `merge.start` stopped
    /// on conflict.
    pub fn set_operation(&mut self, operation: Option<Operation>) {
        self.operation = operation;
        self.merge_state = None;
        self.rebase_state = None;
        self.squash_state = None;
    }

    fn set_ref(&mut self, name: String, target: Oid, tag_object: Option<Oid>) {
        self.refs.insert(name, RefEntry { target, tag_object });
    }

    fn head_target(&self) -> Option<Oid> {
        match &self.head {
            HeadRef::Branch(name) => self.refs.get(&format!("{HEADS}{name}")).map(|r| r.target),
            HeadRef::Detached(oid) => Some(*oid),
        }
    }

    /// Resolves a rev like `git rev-parse`, in git's order: `HEAD`, full name,
    /// `refs/<rev>`, tag, local branch, remote branch, then id (also abbreviated).
    fn resolve(&self, rev: &str) -> Result<Oid, EngineError> {
        let unknown = || EngineError::UnknownRev(rev.to_owned());
        if rev == "HEAD" {
            return self.head_target().ok_or_else(unknown);
        }
        let candidates = [
            rev.to_owned(),
            format!("refs/{rev}"),
            format!("{TAGS}{rev}"),
            format!("{HEADS}{rev}"),
            format!("{REMOTES}{rev}"),
        ];
        if let Some(entry) = candidates.iter().find_map(|name| self.refs.get(name)) {
            return Ok(entry.target);
        }
        // Commit backups are refs too (`refs/project/backup/...`).
        // A backup of a tag object (`tag.delete`, `remote.delete_tag`) leads to its commit, like `^{commit}`.
        if let Some((_, id)) = self.commit_backups.iter().find(|(name, _)| name == rev) {
            return Ok(self.tag_objects.get(id).copied().unwrap_or(*id));
        }
        let hex = rev.to_ascii_lowercase();
        if (4..=Oid::HEX_LEN).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            let mut matches = self
                .commits
                .keys()
                .filter(|id| id.to_string().starts_with(&hex));
            // An ambiguous prefix does not resolve, as in git.
            if let (Some(id), None) = (matches.next(), matches.next()) {
                return Ok(*id);
            }
        }
        Err(unknown())
    }

    /// Commits reachable from `start` (included), only those present in the repo.
    fn reachable(&self, start: Oid) -> BTreeSet<Oid> {
        self.reachable_from([start])
    }

    fn reachable_from(&self, starts: impl IntoIterator<Item = Oid>) -> BTreeSet<Oid> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<Oid> = starts.into_iter().collect();
        while let Some(id) = stack.pop() {
            if let Some(commit) = self.commits.get(&id)
                && seen.insert(id)
            {
                stack.extend(commit.parents.iter().copied());
            }
        }
        seen
    }

    fn upstream_of(&self, local: &str) -> Option<Upstream> {
        let remote = self.upstreams.get(local)?;
        let (ahead, behind) = match (self.refs.get(local), self.refs.get(remote)) {
            (Some(local), Some(remote)) => {
                let ours = self.reachable(local.target);
                let theirs = self.reachable(remote.target);
                (
                    Some(count(ours.difference(&theirs).count())),
                    Some(count(theirs.difference(&ours).count())),
                )
            }
            // The remote is gone: `gone`.
            _ => (None, None),
        };
        Some(Upstream {
            name: remote.clone(),
            ahead,
            behind,
        })
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn kind_of(name: &str) -> RefKind {
    if name.starts_with(HEADS) {
        RefKind::LocalBranch
    } else if name.starts_with(REMOTES) {
        RefKind::RemoteBranch
    } else if name.starts_with(TAGS) {
        RefKind::Tag
    } else {
        RefKind::Stash
    }
}

/// One side of a file in a diff: mode, id and content (if known).
#[derive(Clone, Copy)]
struct Side<'a> {
    mode: u32,
    oid: Oid,
    content: Option<&'a [u8]>,
}

/// Fake id of the content of a worktree file (git computes it with `hash-object`).
fn content_oid(content: &[u8]) -> Oid {
    // FNV-1a: deterministic and different for different contents (in practice).
    let hash = content.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    MemoryBackend::fake_oid(hash)
}

/// The [`FileDiff`] of a file from `old` (absent for an added file) to `new` (absent for a
/// deleted file), with the fields as `parse_diff` reads them from git's output.
fn file_diff<'a>(
    change: Change,
    old_path: &RepoPath,
    new_path: &RepoPath,
    similarity: Option<u8>,
    old: Option<Side<'a>>,
    new: Option<Side<'a>>,
) -> FileDiff {
    let zero = Oid::from_bytes([0; Oid::LEN]);
    let mut file = FileDiff {
        old_path: old_path.clone(),
        new_path: new_path.clone(),
        change,
        similarity,
        old_mode: old.map(|s| s.mode),
        new_mode: new.map(|s| s.mode),
        old_oid: Some(old.map_or(zero, |s| s.oid)),
        new_oid: Some(new.map_or(zero, |s| s.oid)),
        binary: false,
        hunks: Vec::new(),
    };
    if let (Some(o), Some(n)) = (old, new)
        && o.oid == n.oid
    {
        // Unchanged content (exact rename, mode-only change): git does not write the `index`
        // line, and the modes only if they change.
        file.old_oid = None;
        file.new_oid = None;
        if o.mode == n.mode {
            file.old_mode = None;
            file.new_mode = None;
        }
        return file;
    }
    let content = |side: Option<Side<'a>>| -> Option<&'a [u8]> {
        match side {
            None => Some(&[][..]),
            Some(s) if s.oid == zero => Some(&[][..]),
            Some(s) => s.content,
        }
    };
    match (content(old), content(new)) {
        // Like git: binary if there is a NUL in the first 8000 bytes.
        (Some(o), Some(n)) if !is_binary(o) && !is_binary(n) => {
            file.hunks = line_diff::hunks(o, n);
        }
        _ => file.binary = true,
    }
    file
}

fn is_binary(content: &[u8]) -> bool {
    content.iter().take(8000).any(|&b| b == 0)
}

fn unmerged(path: &RepoPath) -> FileDiff {
    FileDiff {
        old_path: path.clone(),
        new_path: path.clone(),
        change: Change::Unmerged,
        similarity: None,
        old_mode: None,
        new_mode: None,
        old_oid: None,
        new_oid: None,
        binary: false,
        hunks: Vec::new(),
    }
}

/// The kind of change of a file in the diff: a type change is a modification (git writes it
/// as two sections, but one is enough here).
fn diff_change(change: Change) -> Change {
    match change {
        Change::TypeChanged => Change::Modified,
        other => other,
    }
}

impl MemoryBackend {
    fn side(&self, mode: u32, oid: Oid) -> Option<Side<'_>> {
        (mode != 0).then(|| Side {
            mode,
            oid,
            content: self.blobs.get(&oid).map(Vec::as_slice),
        })
    }

    /// The files of commit `rev` relative to the first parent, from the files of `show_commit`.
    fn commit_diff(&self, rev: &str) -> Result<Vec<FileDiff>, EngineError> {
        let details = self.show_commit(rev)?;
        Ok(self.changes_diff(&details.files))
    }

    /// Commit `to` relative to commit `from`, from their trees. Renames: only exact ones (a
    /// deleted file and an added file with the same blob), paired by path order.
    fn range_diff(&self, from: Oid, to: Oid) -> Vec<FileDiff> {
        // `from` is the first parent: the changes of the commit (with its renames), as `diff.commit`.
        if self.commits.get(&to).and_then(|c| c.parents.first()) == Some(&from) {
            return self.changes_diff(self.changes.get(&to).map_or(&[], Vec::as_slice));
        }
        let mut changes = commit::tree_changes(&self.tree_at(from), &self.tree_at(to));
        let mut i = 0;
        while i < changes.len() {
            let added = &changes[i];
            let source = (added.change == Change::Added)
                .then(|| {
                    changes.iter().position(|d| {
                        d.change == Change::Deleted
                            && d.old_oid == added.new_oid
                            && d.old_mode == added.new_mode
                    })
                })
                .flatten();
            match source {
                Some(d) => {
                    let deleted = changes.remove(d);
                    let i_now = if d < i { i - 1 } else { i };
                    let file = &mut changes[i_now];
                    file.change = Change::Renamed;
                    file.original = Some(deleted.path);
                    file.score = Some(100);
                    file.old_mode = deleted.old_mode;
                    file.old_oid = deleted.old_oid;
                    i = i_now + 1;
                }
                None => i += 1,
            }
        }
        self.changes_diff(&changes)
    }

    /// The [`FileDiff`]s of the files `changes`, with the contents of the known blobs.
    fn changes_diff(&self, changes: &[ChangedFile]) -> Vec<FileDiff> {
        changes
            .iter()
            .map(|f| {
                file_diff(
                    diff_change(f.change),
                    f.old_path(),
                    &f.path,
                    f.score,
                    self.side(f.old_mode, f.old_oid),
                    self.side(f.new_mode, f.new_oid),
                )
            })
            .collect()
    }

    /// Index relative to HEAD, from the status entries.
    fn index_diff(&self) -> Vec<FileDiff> {
        let mut files = Vec::new();
        for entry in &self.entries {
            let (info, original, score) = match &entry.kind {
                EntryKind::Ordinary(info) => (info, None, None),
                EntryKind::Renamed {
                    info,
                    original,
                    score,
                }
                | EntryKind::Copied {
                    info,
                    original,
                    score,
                } => (info, Some(original), Some(*score)),
                EntryKind::Unmerged(_) => {
                    files.push(unmerged(&entry.path));
                    continue;
                }
                EntryKind::Untracked | EntryKind::Ignored => continue,
            };
            if !entry.index.is_change() {
                continue;
            }
            let old = self.side(info.mode_head, info.oid_head);
            let new = match entry.index {
                Change::Deleted => None,
                _ => self.side(info.mode_index, info.oid_index),
            };
            files.push(file_diff(
                diff_change(entry.index),
                original.unwrap_or(&entry.path),
                &entry.path,
                score,
                old,
                new,
            ));
        }
        files
    }

    /// Worktree relative to the index, from the status entries and the worktree files.
    fn worktree_diff(&self) -> Vec<FileDiff> {
        let mut files = Vec::new();
        for entry in &self.entries {
            let info = match &entry.kind {
                EntryKind::Ordinary(info)
                | EntryKind::Renamed { info, .. }
                | EntryKind::Copied { info, .. } => info,
                EntryKind::Unmerged(_) => {
                    files.push(unmerged(&entry.path));
                    continue;
                }
                EntryKind::Untracked | EntryKind::Ignored => continue,
            };
            if !entry.worktree.is_change() {
                continue;
            }
            // A file added outside the stage (`git add -N`) has no content in the index.
            let old = match entry.worktree {
                Change::Added => None,
                _ => self.side(info.mode_index, info.oid_index),
            };
            let new = match entry.worktree {
                Change::Deleted => None,
                _ => {
                    let content = self.worktree.get(&entry.path).map(Vec::as_slice);
                    Some(Side {
                        mode: info.mode_worktree,
                        oid: content.map_or_else(|| MemoryBackend::fake_oid(u64::MAX), content_oid),
                        content,
                    })
                }
            };
            files.push(file_diff(
                diff_change(entry.worktree),
                &entry.path,
                &entry.path,
                None,
                old,
                new,
            ));
        }
        files
    }
}

impl GitRead for MemoryBackend {
    fn head(&self) -> Result<Head, EngineError> {
        Ok(match &self.head {
            HeadRef::Branch(name) => Head::Branch {
                name: name.clone(),
                target: self.head_target(),
            },
            HeadRef::Detached(oid) => Head::Detached(*oid),
        })
    }

    fn refs(&self) -> Result<Vec<RefInfo>, EngineError> {
        Ok(self
            .refs
            .iter()
            .map(|(name, entry)| {
                let kind = kind_of(name);
                RefInfo {
                    name: name.clone(),
                    kind,
                    target: entry.target,
                    tag_object: entry.tag_object,
                    upstream: match kind {
                        RefKind::LocalBranch => self.upstream_of(name),
                        _ => None,
                    },
                }
            })
            .collect())
    }

    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>, EngineError> {
        query.check()?;
        let starts: Vec<Oid> = if query.all {
            // Like `--branches --remotes --tags HEAD`: no stash or backup (with `stashes` the
            // stash bases, not the stashes).
            self.refs
                .iter()
                .filter(|(name, _)| [HEADS, REMOTES, TAGS].iter().any(|p| name.starts_with(p)))
                .map(|(_, entry)| entry.target)
                .chain(self.head_target())
                .chain(
                    self.stashes
                        .iter()
                        .filter(|_| query.stashes)
                        .filter_map(|s| s.parents.first().copied()),
                )
                .collect()
        } else {
            match &query.rev {
                Some(rev) => vec![self.resolve(rev)?],
                None => self.head_target().into_iter().collect(),
            }
        };
        let wanted = query.skip.saturating_add(query.limit.unwrap_or(usize::MAX));
        let reachable = self.reachable_from(starts);

        // Like `git log --date-order`: a commit comes out only after all its children; among those
        // ready, the most recent first (on equal date the id decides, for a stable order).
        let mut children: BTreeMap<Oid, usize> = reachable.iter().map(|id| (*id, 0)).collect();
        for id in &reachable {
            for parent in &self.commits[id].parents {
                if let Some(n) = children.get_mut(parent) {
                    *n += 1;
                }
            }
        }
        let key = |id: Oid| (self.commits[&id].committer.time, id);
        let mut ready: BinaryHeap<(i64, Oid)> = children
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(id, _)| key(*id))
            .collect();
        let mut out = Vec::new();
        while out.len() < wanted
            && let Some((_, id)) = ready.pop()
        {
            let commit = &self.commits[&id];
            for parent in &commit.parents {
                if let Some(n) = children.get_mut(parent) {
                    *n -= 1;
                    if *n == 0 {
                        ready.push(key(*parent));
                    }
                }
            }
            out.push(commit);
        }
        Ok(out.into_iter().skip(query.skip).cloned().collect())
    }

    fn search(&self, query: &SearchQuery, job: &JobContext) -> Result<Vec<Oid>, EngineError> {
        self.memory_search(query, job)
    }

    fn status(&self) -> Result<Status, EngineError> {
        let upstream = match &self.head {
            HeadRef::Branch(name) => self.upstream_of(&format!("{HEADS}{name}")).map(|up| {
                // `git status` uses the short name.
                Upstream {
                    name: up.name.strip_prefix(REMOTES).unwrap_or(&up.name).to_owned(),
                    ..up
                }
            }),
            HeadRef::Detached(_) => None,
        };
        Ok(Status {
            head: self.head()?,
            upstream,
            // Untracked files ignored by `.gitignore` or `.git/info/exclude` are not listed.
            entries: self
                .entries
                .iter()
                .filter(|e| !(matches!(e.kind, EntryKind::Untracked) && self.is_ignored(&e.path)))
                .cloned()
                .collect(),
            operation: self.operation,
        })
    }

    fn show_commit(&self, rev: &str) -> Result<CommitDetails, EngineError> {
        let id = self.resolve(rev)?;
        // A ref may point to a commit the repo does not have (as in a shallow clone).
        let commit = self
            .commits
            .get(&id)
            .ok_or_else(|| EngineError::UnknownRev(rev.to_owned()))?;
        Ok(CommitDetails {
            commit: commit.clone(),
            compared_to: commit.parents.first().copied(),
            files: self.changes.get(&id).cloned().unwrap_or_default(),
        })
    }

    fn diff(&self, spec: &DiffSpec) -> Result<Diff, EngineError> {
        let mut files = match spec {
            DiffSpec::Worktree { .. } => self.worktree_diff(),
            DiffSpec::Index { .. } => self.index_diff(),
            DiffSpec::Commit { rev, .. } => self.commit_diff(rev)?,
            DiffSpec::Range { from, to, .. } => self.range_diff(*from, *to),
        };
        if let Some(path) = spec.path() {
            files.retain(|f| f.new_path == *path || f.old_path == *path);
        }
        Ok(Diff { files })
    }

    fn blame(&self, query: &BlameQuery, job: &JobContext) -> Result<FileBlame, EngineError> {
        self.memory_blame(query, job)
    }

    fn file_log(
        &self,
        query: &FileLogQuery,
        job: &JobContext,
    ) -> Result<Vec<FileLogEntry>, EngineError> {
        self.memory_file_log(query, job)
    }

    fn unmerged_count(&self, tip: &Oid, into: &[Oid]) -> Result<usize, EngineError> {
        Ok(self.unmerged(tip, into))
    }

    fn config_values(&self, key: &str) -> Result<Vec<String>, EngineError> {
        Ok(self.config_of(key))
    }

    fn commit_id(&self, rev: &str) -> Result<Option<Oid>, EngineError> {
        Ok(self
            .resolve(rev)
            .ok()
            .filter(|id| self.commits.contains_key(id)))
    }

    fn common_base(&self, a: &Oid, b: &Oid) -> Result<Option<Oid>, EngineError> {
        Ok(self.merge_base(*a, *b))
    }

    fn local_upstream(&self, name: &str) -> Result<Vec<ConfigEntry>, EngineError> {
        Ok(self.upstream_entries(name))
    }
}

impl GitIndex for MemoryBackend {
    fn stage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError> {
        self.stage_paths(paths)
    }

    fn unstage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError> {
        self.unstage_paths(paths)
    }

    fn apply_lines(&mut self, selection: &LineSelection) -> Result<Option<String>, EngineError> {
        self.apply_line_patch(selection)
    }

    fn commit(&mut self, request: &CommitRequest) -> Result<CommitOutcome, EngineError> {
        self.create_commit(request)
    }

    fn discard(
        &mut self,
        paths: &[RepoPath],
        expected: &[DiscardedFile],
    ) -> Result<DiscardOutcome, EngineError> {
        self.discard_paths(paths, expected)
    }

    fn restore_worktree(
        &mut self,
        request: &WorktreeRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        self.restore_discarded(request)?;
        Ok(RestoreOutcome::default())
    }

    fn discard_lines(
        &mut self,
        selection: &super::DiscardLines,
    ) -> Result<super::DiscardLinesOutcome, EngineError> {
        self.discard_line_selection(selection)
    }

    fn restore_lines(
        &mut self,
        request: &super::LinesRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        self.restore_discarded_lines(request)?;
        Ok(RestoreOutcome::default())
    }

    fn clean_plan(&self, paths: &[RepoPath]) -> Result<Vec<super::CleanedFile>, EngineError> {
        self.plan_clean(paths)
    }

    fn clean(
        &mut self,
        paths: &[RepoPath],
        expected: &[super::CleanedFile],
    ) -> Result<super::CleanOutcome, EngineError> {
        self.clean_paths(paths, expected)
    }

    fn restore_cleaned(
        &mut self,
        request: &super::CleanRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        self.restore_clean(request)
    }

    fn ignore(
        &mut self,
        request: &super::IgnoreRequest,
    ) -> Result<super::IgnoreOutcome, EngineError> {
        self.ignore_patterns(request)
    }

    fn restore_ignore(
        &mut self,
        request: &super::IgnoreRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        self.restore_ignore_file(request)
    }
}

impl GitRefs for MemoryBackend {
    fn reset(&mut self, request: &super::ResetRequest) -> Result<super::ResetOutcome, EngineError> {
        self.reset_head(request)
    }

    fn restore_refs(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError> {
        if let Some(budget) = self.restore_budget.as_mut() {
            if *budget == 0 {
                return Err(EngineError::Git {
                    command: "update-ref".to_owned(),
                    stderr: "simulated failure (fail_restore_refs_after)".to_owned(),
                });
            }
            *budget -= 1;
        }
        Ok(RestoreOutcome {
            backups: self.restore(request)?,
            warnings: Vec::new(),
        })
    }

    fn create_branch(&mut self, name: &str, target: &Oid) -> Result<(), EngineError> {
        self.create_local_branch(name, target)
    }

    fn create_tag(
        &mut self,
        name: &str,
        target: &Oid,
        message: Option<&str>,
    ) -> Result<Oid, EngineError> {
        self.create_tag_ref(name, target, message)
    }

    fn reset_branch(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError> {
        self.reset_keep(request)
    }

    fn delete_tag(&mut self, name: &str, expected: &Oid) -> Result<String, EngineError> {
        self.delete_local_tag(name, expected)
    }

    fn checkout(&mut self, target: &HeadState) -> Result<RestoreOutcome, EngineError> {
        self.checkout_head(target)
    }

    fn delete_branch(&mut self, name: &str, expected: &Oid) -> Result<BranchDeleted, EngineError> {
        self.delete_local_branch(name, expected)
    }

    fn rename_branch(
        &mut self,
        from: &str,
        to: &str,
        expected: &Oid,
    ) -> Result<RestoreOutcome, EngineError> {
        self.rename_local_branch(from, to, expected)
    }

    fn restore_branch_config(
        &mut self,
        name: &str,
        entries: &[ConfigEntry],
        remove: bool,
    ) -> Vec<Message> {
        self.set_branch_config(name, entries, remove)
    }

    fn write_upstream(
        &mut self,
        name: &str,
        expected: &[ConfigEntry],
        entries: &[ConfigEntry],
    ) -> Result<Vec<Message>, EngineError> {
        self.replace_upstream(name, expected, entries)
    }
}

impl GitRemotes for MemoryBackend {
    fn remotes(&self) -> Result<Vec<String>, EngineError> {
        let mut names = self.remote_names();
        names.extend(self.listed_anyway.iter().cloned());
        names.sort();
        names.dedup();
        Ok(names)
    }

    fn fetch(
        &mut self,
        request: &FetchRequest,
        job: &JobContext,
    ) -> Result<FetchOutcome, EngineError> {
        self.fetch_remote(request, job)
    }

    fn upstream(&self, branch: &str) -> Result<Option<UpstreamConfig>, EngineError> {
        Ok(self.upstream_config(branch))
    }

    fn pull_settings(&self, _branch: &str) -> Result<PullSettings, EngineError> {
        Ok(self.pull_settings.clone())
    }

    fn missing_commits(
        &self,
        tip: &Oid,
        exclude: &[Oid],
        limit: usize,
    ) -> Result<Vec<Commit>, EngineError> {
        self.commits_missing(tip, exclude, limit)
    }

    fn pull(&mut self, request: &PullRequest) -> Result<PullOutcome, EngineError> {
        self.pull_upstream(request)
    }

    fn push(
        &mut self,
        request: &PushRequest,
        job: &JobContext,
    ) -> Result<PushOutcome, EngineError> {
        self.push_branch(request, job)
    }

    fn delete_remote_branch(
        &mut self,
        request: &RemoteBranchDelete,
        job: &JobContext,
    ) -> Result<RemoteBranchDeleted, EngineError> {
        self.delete_branch_on_remote(request, job)
    }

    fn remote_tag(
        &self,
        remote: &str,
        name: &str,
        job: &JobContext,
    ) -> Result<Option<Oid>, EngineError> {
        self.tag_value_on_remote(remote, name, job)
    }

    fn push_tag(&mut self, request: &TagPush, job: &JobContext) -> Result<TagPushed, EngineError> {
        self.push_tag_to_remote(request, job)
    }

    fn delete_remote_tag(
        &mut self,
        request: &RemoteTagDelete,
        job: &JobContext,
    ) -> Result<RemoteBranchDeleted, EngineError> {
        self.delete_tag_on_remote(request, job)
    }

    fn remote_snapshot(&self, name: &str) -> Result<RemoteSnapshot, EngineError> {
        Ok(self.snapshot_of(name))
    }

    fn create_remote(&mut self, name: &str, url: &str) -> Result<(), EngineError> {
        self.create_memory_remote(name, url)
    }

    fn remove_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<Message> {
        self.drop_remote_config(snapshot)
    }

    fn write_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<Message> {
        self.put_remote_config(snapshot)
    }

    fn remote_config_elsewhere(&self, name: &str) -> Result<Vec<String>, EngineError> {
        Ok(self.remote_elsewhere.get(name).cloned().unwrap_or_default())
    }

    fn set_remote_url(
        &mut self,
        name: &str,
        push: bool,
        url: Option<&str>,
    ) -> Result<(), EngineError> {
        self.change_remote_url(name, push, url)
    }

    fn rename_remote(&mut self, from: &str, to: &str) -> Result<Vec<Message>, EngineError> {
        self.rename_memory_remote(from, to)
    }
}

impl GitStash for MemoryBackend {
    fn stashes(&self) -> Result<Vec<StashEntry>, EngineError> {
        Ok(self.stash_list())
    }

    fn stash_push(&mut self, request: &StashPush) -> Result<StashEntry, EngineError> {
        self.push_stash(request)
    }

    fn stash_apply(&mut self, request: &StashApply) -> Result<StashApplied, EngineError> {
        self.apply_stash(request)
    }

    fn stash_drop(&mut self, request: &StashDrop) -> Result<StashDropped, EngineError> {
        self.drop_stash(request)
    }

    fn stash_store(&mut self, id: &Oid, message: &str) -> Result<(), EngineError> {
        self.store_stash(id, message)
    }
}

impl GitOperations for MemoryBackend {
    fn conflict_versions(&self, path: &RepoPath) -> Result<ConflictVersions, EngineError> {
        self.conflict_files(path)
    }

    fn resolve_conflict(
        &mut self,
        request: &ConflictResolution,
    ) -> Result<ConflictResolved, EngineError> {
        self.resolve_file(request)
    }

    fn merge(&mut self, request: &MergeRequest) -> Result<MergeOutcome, EngineError> {
        self.merge_commit(request)
    }

    fn merge_abort(&mut self) -> Result<MergeAborted, EngineError> {
        self.abort_merge()
    }

    fn operation_state(&self) -> Result<OperationState, EngineError> {
        Ok(self.operation_info())
    }

    fn pick(&mut self, request: &PickRequest) -> Result<MergeOutcome, EngineError> {
        self.pick_commit(request)
    }

    fn pick_abort(&mut self, kind: PickKind) -> Result<MergeAborted, EngineError> {
        self.abort_pick(kind)
    }

    fn rebase(&mut self, request: &RebaseRequest) -> Result<MergeOutcome, EngineError> {
        self.rebase_start(request)
    }

    fn rebase_continue(&mut self) -> Result<MergeOutcome, EngineError> {
        self.rebase_go_on()
    }

    fn rebase_abort(&mut self) -> Result<MergeAborted, EngineError> {
        self.rebase_stop()
    }
}

impl super::GitSubmodules for MemoryBackend {
    fn submodules(&self) -> Result<Vec<super::SubmoduleInfo>, EngineError> {
        Ok(self.submodules.clone())
    }

    fn update_submodules(
        &mut self,
        request: &super::SubmoduleUpdate,
        job: &JobContext,
    ) -> Result<super::SubmoduleUpdated, EngineError> {
        self.memory_submodule_update(request, job)
    }
}

impl GitMaintenance for MemoryBackend {
    fn commit_graph(&self) -> Result<CommitGraphState, EngineError> {
        // Like git: the last value of `core.commitGraph` wins; `false`, `no`, `off`, `0` and
        // the empty value disable it.
        let enabled = self.config_of("core.commitGraph").last().is_none_or(|v| {
            !matches!(
                v.to_ascii_lowercase().as_str(),
                "false" | "no" | "off" | "0" | ""
            )
        });
        Ok(CommitGraphState {
            present: self.commit_graph,
            enabled,
            shallow: false,
            replaced: false,
        })
    }

    fn write_commit_graph(&mut self, job: &JobContext) -> Result<(), EngineError> {
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "repo.optimize",
            });
        }
        self.commit_graph = true;
        Ok(())
    }
}

/// [`RepoOpener`] over in-memory repos, registered by path. Each `open` returns an independent
/// copy of the registered repo. `repo.clone` and `repo.init` register the new repo at their
/// `path` (`impl-docs/git/clone.md`): a clone copies the repo registered at the `url`.
#[derive(Debug, Default)]
pub struct MemoryOpener {
    state: Mutex<OpenerState>,
}

#[derive(Debug, Clone, Default)]
struct OpenerState {
    repos: BTreeMap<String, MemoryBackend>,
    /// Paths that are a non-empty folder (or a file) without being a repo: clone and init
    /// refuse them.
    occupied: BTreeSet<String>,
    /// URLs whose clone fails with [`EngineError::Auth`], with git's message.
    denied: BTreeMap<String, String>,
}

impl Clone for MemoryOpener {
    fn clone(&self) -> Self {
        Self {
            state: Mutex::new(self.lock().clone()),
        }
    }
}

impl MemoryOpener {
    /// Path of the sample repo in [`MemoryOpener::sample`].
    pub const SAMPLE_PATH: &'static str = "sample";

    /// No registered repo.
    pub fn new() -> Self {
        Self::default()
    }

    /// Only the repo from [`MemoryBackend::sample`], at path [`MemoryOpener::SAMPLE_PATH`].
    pub fn sample() -> Self {
        let mut opener = Self::new();
        opener.insert(Self::SAMPLE_PATH, MemoryBackend::sample());
        opener
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OpenerState> {
        // A panic while holding the lock leaves a consistent map: every write is one insert.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Registers (or replaces) the repo at path `path`.
    pub fn insert(&mut self, path: impl Into<String>, repo: MemoryBackend) {
        self.lock().repos.insert(path.into(), repo);
    }

    /// Registered paths, in order (also those created by `repo.clone` and `repo.init`).
    pub fn paths(&self) -> Vec<String> {
        self.lock().repos.keys().cloned().collect()
    }

    /// A copy of the repo registered at `path`, if any.
    pub fn repo(&self, path: &str) -> Option<MemoryBackend> {
        self.lock().repos.get(path).cloned()
    }

    /// `path` is a non-empty folder (or a file) that is not a repo: clone and init refuse it.
    pub fn occupy(&mut self, path: impl Into<String>) {
        self.lock().occupied.insert(path.into());
    }

    /// The clone of `url` fails with [`EngineError::Auth`] and `message`.
    pub fn deny(&mut self, url: impl Into<String>, message: impl Into<String>) {
        self.lock().denied.insert(url.into(), message.into());
    }

    /// The destination of a clone or init: neither a repo nor an occupied path.
    fn check_destination(state: &OpenerState, path: &str) -> Result<(), EngineError> {
        if state.repos.contains_key(path) || state.occupied.contains(path) {
            return Err(super::destination_not_empty(path));
        }
        Ok(())
    }
}

impl RepoOpener for MemoryOpener {
    fn open(&self, path: &str) -> Result<Box<dyn GitBackend>, EngineError> {
        let state = self.lock();
        match state.repos.get(path) {
            Some(repo) => Ok(Box::new(repo.clone())),
            None => {
                let known: Vec<&str> = state.repos.keys().map(String::as_str).collect();
                Err(EngineError::RepoNotFound {
                    path: path.to_owned(),
                    reason: if known.is_empty() {
                        msg!("engine-memory-no-repo")
                    } else {
                        msg!("engine-memory-repos", repos = known.join(", "))
                    },
                })
            }
        }
    }

    fn clone_repo(
        &self,
        request: &CloneRequest,
        job: &JobContext,
    ) -> Result<CloneOutcome, EngineError> {
        let mut state = self.lock();
        Self::check_destination(&state, &request.path)?;
        if let Some(message) = state.denied.get(&request.url) {
            return Err(EngineError::Auth {
                remote: request.url.clone(),
                message: message.clone(),
            });
        }
        let Some(remote) = state.repos.get(&request.url) else {
            return Err(EngineError::RemoteUnreachable {
                remote: request.url.clone(),
                message: format!("fatal: repository '{}' does not exist", request.url),
            });
        };
        let mut clone = remote.cloned(&request.url, request.branch.as_deref())?;
        clone.clone_submodules(&remote.submodules, request.submodules);
        job.report(&Progress {
            phase: "Receiving objects".to_owned(),
            remote: false,
            percent: Some(100),
            current: None,
            total: None,
            done: true,
            text: "Receiving objects: 100%, done.".to_owned(),
        });
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "repo.clone",
            });
        }
        let mut messages = Vec::new();
        if clone.commits.is_empty() {
            messages.push("warning: You appear to have cloned an empty repository.".to_owned());
        }
        state.repos.insert(request.path.clone(), clone);
        Ok(CloneOutcome { messages })
    }

    fn init_repo(&self, request: &InitRequest) -> Result<(), EngineError> {
        let mut state = self.lock();
        Self::check_destination(&state, &request.path)?;
        state.repos.insert(
            request.path.clone(),
            MemoryBackend::initialized(request.branch.as_deref()),
        );
        Ok(())
    }
}
