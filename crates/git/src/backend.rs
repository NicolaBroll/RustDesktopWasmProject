//! [`CliBackend`] and [`CliOpener`]: reading a repository by running `git`.

mod branch;
mod clone;
mod commit;
mod conflict;
mod discard;
mod file;
mod maintenance;
mod merge;
mod pick;
mod rebase;
mod remote;
mod remote_admin;
mod reset;
mod restore;
mod search;
mod stash;
mod sync;
mod tag;

use crate::GitVersion;
use crate::command::{Git, failure, os_from_bytes, path_from_output, stderr_text};
use project_domain::EntryKind;
use project_domain::branch_short;
use project_domain::{
    Change, Commit, CommitDetails, Diff, FileLogEntry, Head, RefInfo, RepoPath, Status,
};
use project_domain::{
    DIFF_ARGS, DIFF_TREE_ARGS, LOG_ARGS, Oid, Operation, PatchDirection, REFS_ARGS, STATUS_ARGS,
    parse_diff, parse_diff_tree, parse_log, parse_refs, parse_status,
};
use project_engine::{
    BACKUP_PREFIX, BlameQuery, BranchDeleted, CloneOutcome, CloneRequest, CommitGraphState,
    CommitOutcome, CommitRequest, ConfigEntry, ConflictResolution, ConflictResolved,
    ConflictVersions, DiffSpec, DiscardOutcome, DiscardedFile, EngineError, FetchOutcome,
    FetchRequest, FileBlame, FileLogQuery, GitBackend, GitIndex, GitMaintenance, GitOperations,
    GitRead, GitRefs, GitRemotes, GitStash, HeadState, InitRequest, JobContext, LinePatch,
    LineSelection, LogQuery, MergeAborted, MergeOutcome, MergeRequest, Message, OperationState,
    PickKind, PickRequest, PullOutcome, PullRequest, PullSettings, PushOutcome, PushRequest,
    RebaseRequest, RefRestore, RemoteBranchDelete, RemoteBranchDeleted, RemoteSnapshot,
    RemoteTagDelete, RepoOpener, RestoreOutcome, SearchQuery, StagedVersion, StashApplied,
    StashApply, StashDrop, StashDropped, StashEntry, StashPush, TagPush, TagPushed, UpstreamConfig,
    WorktreeRestore, apply_patch, backup_message, line_patch, path_matches,
    versions_at_risk_hidden,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Opens repositories with the system `git`. It is the [`RepoOpener`] of the desktop and the CLI.
///
/// On every open it checks that git exists and that its version is at least [`GitVersion::MIN`].
#[derive(Debug, Clone)]
pub struct CliOpener {
    git: Git,
}

impl Default for CliOpener {
    fn default() -> Self {
        Self::new()
    }
}

impl CliOpener {
    /// Uses `git` from the `PATH`.
    pub fn new() -> Self {
        Self::with_program("git")
    }

    /// Uses the given program instead of `git` (e.g. an absolute path).
    pub fn with_program(program: impl Into<OsString>) -> Self {
        Self {
            git: Git::new(program),
        }
    }

    /// Adds an environment variable to every git run, including those of already opened backends.
    /// Used by tests for a deterministic environment (e.g. `GIT_CONFIG_GLOBAL=/dev/null`).
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.git.set_env(key.into(), value.into());
        self
    }

    /// Does not search for repositories above `root`: when opening a directory inside `root` that is not a repo,
    /// git does not climb up to a repo that contains `root` (`GIT_CEILING_DIRECTORIES`). A repo
    /// in `root` or below opens as usual. Used by the MCP server to stay within the root; `root` must be absolute and without symbolic
    /// links (`canonicalize`).
    pub fn confine_to(self, root: &Path) -> Self {
        match root.parent() {
            Some(parent) => self.env("GIT_CEILING_DIRECTORIES", parent),
            None => self,
        }
    }

    /// The version of the configured git. [`EngineError::GitNotFound`] if it does not start.
    pub fn git_version(&self) -> Result<GitVersion, EngineError> {
        let out = self.git.run(None, &["--version"])?;
        let text = String::from_utf8_lossy(&out);
        GitVersion::parse(&text).ok_or_else(|| {
            EngineError::GitNotFound(msg!("engine-git-version-unknown", output = text.trim()))
        })
    }

    /// [`EngineError::GitTooOld`] if git is older than [`GitVersion::MIN`].
    fn check_version(&self) -> Result<(), EngineError> {
        let version = self.git_version()?;
        if !version.is_supported() {
            return Err(EngineError::GitTooOld {
                found: version.to_string(),
                required: GitVersion::MIN.to_string(),
            });
        }
        Ok(())
    }

    /// Like [`RepoOpener::open`], but returns the concrete type.
    pub fn open_cli(&self, path: &str) -> Result<CliBackend, EngineError> {
        self.check_version()?;
        let not_found = |reason: Message| EngineError::RepoNotFound {
            path: path.to_owned(),
            reason,
        };
        // `git -C ""` would stay in the current directory.
        if path.is_empty() {
            return Err(not_found(msg!("engine-open-empty-path")));
        }
        let dir = Path::new(path);

        let args = [
            "rev-parse",
            "--is-bare-repository",
            "--absolute-git-dir",
            "--show-object-format",
        ];
        let out = self.git.output(Some(dir), &args)?;
        if !out.status.success() {
            return Err(not_found(msg!("engine-other", text = stderr_text(&out))));
        }
        let lines: Vec<&[u8]> = out.stdout.split(|&b| b == b'\n').collect();
        let [bare, git_dir, format, b""] = lines[..] else {
            return Err(failure_text(&args, &out.stdout));
        };
        if bare == b"true" {
            return Err(not_found(msg!("engine-open-bare")));
        }
        if format != b"sha1" {
            return Err(not_found(msg!(
                "engine-open-object-format",
                format = String::from_utf8_lossy(format).into_owned()
            )));
        }
        let empty_tree = empty_tree(format).ok_or_else(|| {
            not_found(msg!(
                "engine-open-object-format-unknown",
                format = String::from_utf8_lossy(format).into_owned()
            ))
        })?;
        let empty_blob = empty_blob(format).ok_or_else(|| {
            not_found(msg!(
                "engine-open-object-format-unknown",
                format = String::from_utf8_lossy(format).into_owned()
            ))
        })?;
        let git_dir = path_from_output(git_dir);

        // Fails, for example, inside the `.git` directory.
        let out = self
            .git
            .output(Some(dir), &["rev-parse", "--show-toplevel"])?;
        if !out.status.success() {
            return Err(not_found(msg!("engine-other", text = stderr_text(&out))));
        }
        let workdir = path_from_output(&out.stdout);

        Ok(CliBackend {
            git: self.git.clone(),
            workdir,
            git_dir,
            empty_tree,
            empty_blob,
        })
    }
}

/// Id of the empty tree in the object format `format` (`rev-parse --show-object-format`):
/// git always knows it, even if it is not in the repo. Used to compare a root commit and to
/// unstage on a branch with no commits. For now the parsers only support sha1, but
/// the id must never be taken for granted.
fn empty_tree(format: &[u8]) -> Option<&'static str> {
    match format {
        b"sha1" => Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904"),
        b"sha256" => Some("6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321"),
        _ => None,
    }
}

/// Id of the empty blob in the object format `format`: it is the id of the index entry of an
/// `intent-to-add` file (`git add -N`). Today `Oid` is SHA-1 only and a SHA-256 repo is not
/// opened: the sha256 branch is not needed yet, it is there to avoid assuming the format.
fn empty_blob(format: &[u8]) -> Option<&'static str> {
    match format {
        b"sha1" => Some("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"),
        b"sha256" => Some("473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813"),
        _ => None,
    }
}

/// A stage-0 index entry, from `git ls-files -v -s`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexEntry {
    /// Letter from `-v`: lowercase for `--assume-unchanged`, `S` for `--skip-worktree`.
    tag: u8,
    mode: u32,
    oid: Oid,
}

type IndexListing = BTreeMap<RepoPath, IndexEntry>;

/// Identity of the index backup commits: the user's is not needed, and this way the backup
/// succeeds even if `user.name`/`user.email` are not configured.
const BACKUP_IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Project"),
    ("GIT_AUTHOR_EMAIL", "backup@project.invalid"),
    ("GIT_COMMITTER_NAME", "Project"),
    ("GIT_COMMITTER_EMAIL", "backup@project.invalid"),
];

/// A temporary index in the git directory, deleted (with its lock) when it leaves
/// scope. Used to build the backup tree without touching the real index.
struct TempIndex(PathBuf);

impl TempIndex {
    fn new(git_dir: &Path) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Self(git_dir.join(format!(
            "project-backup-index-{}-{nanos}",
            std::process::id()
        )))
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut lock = self.0.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(lock);
    }
}

/// [`EngineError::Git`] for a command that succeeded but with unexpected output.
fn failure_text(args: &[&str], stdout: &[u8]) -> EngineError {
    EngineError::GitWithReason {
        command: args[0].to_owned(),
        reason: msg!(
            "engine-git-unexpected-output",
            output = String::from_utf8_lossy(stdout).trim()
        ),
    }
}

impl RepoOpener for CliOpener {
    fn open(&self, path: &str) -> Result<Box<dyn GitBackend>, EngineError> {
        Ok(Box::new(self.open_cli(path)?))
    }

    fn clone_repo(
        &self,
        request: &CloneRequest,
        job: &JobContext,
    ) -> Result<CloneOutcome, EngineError> {
        self.clone_into(request, job)
    }

    fn init_repo(&self, request: &InitRequest) -> Result<(), EngineError> {
        self.init_into(request)
    }
}

/// A repository read with the system `git`, created by [`CliOpener`].
///
/// Every read runs git in the worktree root and parses the output with the domain parsers.
#[derive(Debug, Clone)]
pub struct CliBackend {
    git: Git,
    workdir: PathBuf,
    git_dir: PathBuf,
    /// Id of the empty tree in the repo's format.
    empty_tree: &'static str,
    /// Id of the empty blob in the repo's format (the entry of an `intent-to-add` file).
    empty_blob: &'static str,
}

impl CliBackend {
    /// Worktree root (`git rev-parse --show-toplevel`).
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Git directory of this worktree (`git rev-parse --absolute-git-dir`).
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    fn run(&self, args: &[&str]) -> Result<Vec<u8>, EngineError> {
        self.git.run(Some(&self.workdir), args)
    }

    /// The commit `rev` leads to, `None` if it does not resolve (even if ambiguous).
    fn resolve(&self, rev: &str) -> Result<Option<Oid>, EngineError> {
        let spec = format!("{rev}^{{commit}}");
        let args = [
            "rev-parse",
            "--quiet",
            "--verify",
            "--end-of-options",
            &spec,
        ];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            let hex = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
            return Ok(Some(Oid::from_hex(hex)?));
        }
        // With `--quiet` an unknown rev writes nothing; anything else on stderr is a real error.
        if out.stderr.iter().all(u8::is_ascii_whitespace) {
            Ok(None)
        } else {
            Err(failure(&args, &out))
        }
    }

    /// The first parent of commit `id`, `None` for a root commit.
    fn first_parent(&self, id: &str) -> Result<Option<Oid>, EngineError> {
        // A line `<commit> <parents>...`.
        let args = [
            "rev-list",
            "--max-count=1",
            "--parents",
            "--end-of-options",
            id,
            "--",
        ];
        let out = self.run(&args)?;
        let mut ids = out
            .split(|b| b.is_ascii_whitespace())
            .filter(|t| !t.is_empty());
        ids.next();
        match ids.next() {
            Some(hex) => Ok(Some(Oid::from_hex(hex)?)),
            None => Ok(None),
        }
    }

    /// `git diff` with `compare` (what to compare, already with `--end-of-options` if revisions
    /// are needed) and `extra` (options), limited to `paths`, taken literally: no globs or
    /// magic pathspecs (`--literal-pathspecs`). A read: it does not update the index. For
    /// `git diff` `--no-optional-locks` is not enough: with a file changed only in its
    /// `stat` data (e.g. rewritten identically) it rewrites the index to update them; this is prevented by
    /// `diff.autoRefreshIndex=false`, and the file stays out of the diff anyway.
    fn run_diff(
        &self,
        extra: &[&str],
        compare: &[&str],
        paths: &[&RepoPath],
    ) -> Result<Vec<u8>, EngineError> {
        let mut args = vec![
            "--no-optional-locks",
            "-c",
            "diff.autoRefreshIndex=false",
            "--literal-pathspecs",
            "diff",
        ];
        args.extend_from_slice(extra);
        args.extend_from_slice(compare);
        args.push("--");
        let paths: Vec<OsString> = paths.iter().map(|p| os_from_bytes(p.as_bytes())).collect();
        let paths: Vec<&OsStr> = paths.iter().map(OsString::as_os_str).collect();
        self.git.run_with_paths(Some(&self.workdir), &args, &paths)
    }

    /// The paths to pass to `git diff` for the file `path`: also the other path if the
    /// file is renamed (from or to `path`), otherwise git, limited to a single path, would not
    /// see the rename but an added or deleted file.
    fn diff_paths(&self, compare: &[&str], path: &RepoPath) -> Result<Vec<RepoPath>, EngineError> {
        // Only renames (`--diff-filter=R`), in the `diff-tree` format.
        let raw = [
            "-z",
            "--raw",
            "--no-abbrev",
            "--find-renames",
            "--diff-filter=R",
        ];
        let renames = parse_diff_tree(&self.run_diff(&raw, compare, &[])?)?;
        let mut paths = vec![path.clone()];
        for file in renames {
            if file.change != Change::Renamed {
                continue;
            }
            let old = file.old_path().clone();
            if file.path == *path {
                paths.push(old);
            } else if old == *path {
                paths.push(file.path);
            }
        }
        Ok(paths)
    }

    /// True if `path` exists in the worktree (even as a broken symbolic link or directory).
    fn in_worktree(&self, path: &RepoPath) -> bool {
        std::fs::symlink_metadata(self.workdir.join(os_from_bytes(path.as_bytes()))).is_ok()
    }

    /// `paths` without those that only indicate deletions already staged: such a file is no
    /// longer in the index nor in the worktree and `git add` would refuse it ("did not match any
    /// files"), even though there is nothing to do. It applies to a file (for example the old
    /// path of a staged rename) and to a directory that contains only staged deletions.
    fn without_staged_deletions(&self, status: &Status, paths: &[RepoPath]) -> Vec<RepoPath> {
        // (path, is a staged deletion) for every status entry.
        let mut items: Vec<(&RepoPath, bool)> = Vec::new();
        for entry in &status.entries {
            match &entry.kind {
                EntryKind::Ordinary(_) => {
                    items.push((&entry.path, entry.index == Change::Deleted));
                }
                EntryKind::Renamed { original, .. } => {
                    items.push((original, true));
                    items.push((&entry.path, false));
                }
                _ => items.push((&entry.path, false)),
            }
        }
        paths
            .iter()
            .filter(|p| {
                let mut matched = items.iter().filter(|(path, _)| path_matches(path, p));
                let mut any = false;
                let only_deletions = matched.all(|(_, deleted)| {
                    any = true;
                    *deleted
                });
                !(any && only_deletions && !self.in_worktree(p))
            })
            .cloned()
            .collect()
    }

    /// Where `unstage` restarts the index from: the HEAD commit, or the empty tree if HEAD
    /// is a branch that has no commits yet. A HEAD that does not resolve for other reasons (broken
    /// ref, missing object) is an error: treating it as "no commits" would remove all the requested files
    /// from the index.
    fn index_source(&self) -> Result<String, EngineError> {
        if let Some(head) = self.resolve("HEAD")? {
            return Ok(head.to_string());
        }
        let out = self
            .git
            .output(Some(&self.workdir), &["symbolic-ref", "--quiet", "HEAD"])?;
        if out.status.success() {
            let name =
                String::from_utf8_lossy(out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout))
                    .into_owned();
            let exists = self
                .git
                .output(
                    Some(&self.workdir),
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        "--end-of-options",
                        &name,
                    ],
                )?
                .status
                .success();
            if !exists {
                return Ok(self.empty_tree.to_owned());
            }
        }
        Err(EngineError::GitWithReason {
            command: "rev-parse".to_owned(),
            reason: msg!("engine-lines-head-invalid"),
        })
    }

    /// Before rewriting the index for `operation` on `paths`: if there are staged versions
    /// that exist only in the index ([`versions_at_risk_hidden`]) saves them with [`Self::save_backup`]
    /// and returns the ref; `None` if nothing is at risk.
    fn backup(
        &self,
        operation: &str,
        status: &Status,
        index: &IndexListing,
        paths: &[RepoPath],
    ) -> Result<Option<String>, EngineError> {
        // With `--assume-unchanged` (lowercase letter) or `--skip-worktree` (`S`) git does not
        // look at the worktree and `status` gives Y = `.` even if the file differs.
        let hidden: BTreeSet<RepoPath> = index
            .iter()
            .filter(|(_, e)| e.tag.is_ascii_lowercase() || e.tag == b'S')
            .map(|(p, _)| p.clone())
            .collect();
        let versions = versions_at_risk_hidden(status, paths, &hidden);
        if versions.is_empty() {
            return Ok(None);
        }
        self.save_backup(operation, &versions).map(Some)
    }

    /// Saves `versions` as in §7: a tree with those files (built in a temporary index,
    /// so it also works with conflicts in the real index), a commit with parent HEAD (if
    /// any) and a new ref `refs/project/backup/index-<seconds>-<nanoseconds>`, created only if
    /// it does not exist (`update-ref <ref> <id> ""`): an existing ref is never overwritten. The
    /// content is recovered with `git show <ref>:<path>`.
    fn save_backup(
        &self,
        operation: &str,
        versions: &[StagedVersion],
    ) -> Result<String, EngineError> {
        let tree = self.backup_tree(versions)?;
        let message = backup_message(operation, versions);
        self.save_backup_commit(operation, &tree, &message)
    }

    /// The tree that contains only `versions` (mode and id at their paths), built in a
    /// temporary index: the real index is not touched (it also works with conflicts).
    pub(super) fn backup_tree(&self, versions: &[StagedVersion]) -> Result<String, EngineError> {
        let index = TempIndex::new(&self.git_dir);
        let index_env = [("GIT_INDEX_FILE", index.0.as_os_str())];
        let mut info = Vec::new();
        for v in versions {
            info.extend_from_slice(format!("{:o} {}\t", v.mode, v.oid).as_bytes());
            info.extend_from_slice(v.path.as_bytes());
            info.push(0);
        }
        // The temporary index is simple: no split index (it would leave a `sharedindex.*` for
        // every backup), fsmonitor or sparse index, whatever the repo config.
        let plain = [
            "-c",
            "core.splitIndex=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "index.sparse=false",
        ];
        let update = [&plain[..], &["update-index", "-z", "--index-info"]].concat();
        self.run_input(&update, &info, &index_env)?;
        let write = [&plain[..], &["write-tree"]].concat();
        let tree = self.run_input(&write, b"", &index_env)?;
        Ok(String::from_utf8_lossy(tree.trim_ascii_end()).into_owned())
    }

    /// Continues [`Self::save_backup`]: creates the commit and the ref.
    fn save_backup_commit(
        &self,
        operation: &str,
        tree: &str,
        message: &str,
    ) -> Result<String, EngineError> {
        let head = self.resolve("HEAD")?.map(|h| h.to_string());
        let mut args = vec!["commit-tree", "--no-gpg-sign"];
        if let Some(head) = &head {
            args.extend(["-p", head]);
        }
        args.extend(["-F", "-", tree]);
        let identity: Vec<(&str, &OsStr)> = BACKUP_IDENTITY
            .iter()
            .map(|(k, v)| (*k, OsStr::new(v)))
            .collect();
        let commit = self.run_input(&args, message.as_bytes(), &identity)?;
        let commit = String::from_utf8_lossy(commit.trim_ascii_end()).into_owned();

        let reflog = format!("project: backup of the index before {operation}");
        self.create_backup_ref("index", &commit, &reflog)
    }

    /// A name `refs/project/backup/<kind>-<seconds>-<nanoseconds>` (with a `-<n>` suffix if
    /// taken) that no ref has now, for a backup written by another command (a fetch straight
    /// into it); whoever writes it must still refuse to overwrite.
    fn free_backup_name(&self, kind: &str) -> Result<String, EngineError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let base = format!(
            "{BACKUP_PREFIX}{kind}-{}-{:09}",
            now.as_secs(),
            now.subsec_nanos()
        );
        for attempt in 0..10 {
            let name = match attempt {
                0 => base.clone(),
                n => format!("{base}-{n}"),
            };
            if self.ref_value(&name)?.is_none() {
                return Ok(name);
            }
        }
        Err(EngineError::GitWithReason {
            command: "update-ref".to_owned(),
            reason: msg!("engine-backup-no-name", base = base),
        })
    }

    /// Creates a new ref `refs/project/backup/<kind>-<seconds>-<nanoseconds>` pointing to
    /// `commit`, only if it does not exist (`update-ref <ref> <id> ""`): an existing ref is
    /// never overwritten; if the name is taken a `-<n>` suffix is tried. Returns the name.
    fn create_backup_ref(
        &self,
        kind: &str,
        commit: &str,
        reflog: &str,
    ) -> Result<String, EngineError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let base = format!(
            "{BACKUP_PREFIX}{kind}-{}-{:09}",
            now.as_secs(),
            now.subsec_nanos()
        );
        for attempt in 0..10 {
            let name = match attempt {
                0 => base.clone(),
                n => format!("{base}-{n}"),
            };
            let args = ["update-ref", "-m", reflog, &name, commit, ""];
            let out = self.git.output(Some(&self.workdir), &args)?;
            if out.status.success() {
                return Ok(name);
            }
            // Only if the name is already taken is the next one tried.
            let taken = self
                .git
                .output(
                    Some(&self.workdir),
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        "--end-of-options",
                        &name,
                    ],
                )?
                .status
                .success();
            if !taken {
                return Err(failure(&args, &out));
            }
        }
        Err(EngineError::GitWithReason {
            command: "update-ref".to_owned(),
            reason: msg!("engine-backup-no-name", base = base),
        })
    }

    /// The status read before a write: like [`GitRead::status`], but without
    /// `core.fsmonitor`, which if out of date makes a changed file look unchanged.
    fn status_for_write(&self) -> Result<Status, EngineError> {
        let mut args = vec![
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "status",
        ];
        args.extend_from_slice(STATUS_ARGS);
        let mut status = parse_status(&self.run(&args)?)?;
        status.operation = self.operation();
        Ok(status)
    }

    /// The stage-0 index entries (`git ls-files -v -s -z`), by path.
    fn index_listing(&self) -> Result<IndexListing, EngineError> {
        let args = ["ls-files", "-v", "-s", "-z", "--"];
        let out = self.run(&args)?;
        let mut listing = BTreeMap::new();
        for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            // `<letter> <mode> <id> <stage>\t<path>`
            let bad = || failure_text(&args, record);
            let tab = record.iter().position(|&b| b == b'\t').ok_or_else(bad)?;
            let (head, path) = (&record[..tab], &record[tab + 1..]);
            let fields: Vec<&[u8]> = head.split(|&b| b == b' ').collect();
            let [[tag], mode, oid, stage] = fields[..] else {
                return Err(bad());
            };
            if stage != b"0" {
                continue;
            }
            let mode = std::str::from_utf8(mode)
                .ok()
                .and_then(|m| u32::from_str_radix(m, 8).ok())
                .ok_or_else(bad)?;
            listing.insert(
                RepoPath::from_bytes(path),
                IndexEntry {
                    tag: *tag,
                    mode,
                    oid: Oid::from_hex(oid)?,
                },
            );
        }
        Ok(listing)
    }

    /// Right before writing: the index entries touched by `paths` agree with the status
    /// read at the start and are still those read at the start (`before`, which also feeds the risk check and the backup).
    /// If another git process changed them in the meantime, error without writing.
    fn check_index_unchanged(
        &self,
        status: &Status,
        before: &IndexListing,
        paths: &[RepoPath],
    ) -> Result<(), EngineError> {
        let now = self.index_listing()?;
        let touched = |listing: &IndexListing| -> Vec<(RepoPath, u32, Oid)> {
            listing
                .iter()
                .filter(|(p, _)| paths.iter().any(|w| path_matches(p, w)))
                .map(|(p, e)| (p.clone(), e.mode, e.oid))
                .collect()
        };
        // And the status entries (read even earlier) agree with the index read afterwards.
        let consistent = status.entries.iter().all(|entry| {
            let info = match &entry.kind {
                EntryKind::Ordinary(info)
                | EntryKind::Renamed { info, .. }
                | EntryKind::Copied { info, .. } => info,
                _ => return true,
            };
            if !paths.iter().any(|w| path_matches(&entry.path, w)) {
                return true;
            }
            match before.get(&entry.path) {
                // An `intent-to-add` file (`.A`): `git status` gives mode and id zero, but
                // the index has an entry with the empty blob.
                Some(e) if entry.worktree == Change::Added && info.mode_index == 0 => {
                    self.is_intent_to_add(e)
                }
                Some(e) => info.mode_index == e.mode && info.oid_index == e.oid,
                None => info.mode_index == 0,
            }
        });
        if consistent && touched(before) == touched(&now) {
            Ok(())
        } else {
            Err(EngineError::NotAllowed(msg!("engine-lines-index-changed")))
        }
    }

    /// [`EngineError::IndexLocked`] if the index is locked by another git process (`index.lock`
    /// exists). Only an early check, to avoid saving a useless backup before
    /// a write that would fail: the real write rechecks it anyway (git never
    /// overwrites a lock).
    fn check_index_unlocked(&self) -> Result<(), EngineError> {
        let lock = self.git_dir.join("index.lock");
        if lock.symlink_metadata().is_ok() {
            return Err(EngineError::IndexLocked {
                lock: lock.to_string_lossy().into_owned(),
            });
        }
        Ok(())
    }

    /// Right after reading the diff the patch comes from: the index entry the patch
    /// changes is the one the diff was computed on, i.e. the one read earlier in `index`
    /// (which also feeds the risk check and the backup). The index side of the diff is the
    /// old one for stage (`git diff`), the new one for unstage (`git diff --cached`).
    fn check_diff_matches_index(
        &self,
        index: &IndexListing,
        prepared: &LinePatch,
        direction: PatchDirection,
    ) -> Result<(), EngineError> {
        let file = &prepared.file;
        let entry = index.get(prepared.target());
        let consistent = match direction {
            // A new file in the worktree diff is an `intent-to-add` placeholder: it is in the index,
            // with the empty blob.
            PatchDirection::Stage if file.change == Change::Added => {
                entry.is_some_and(|e| self.is_intent_to_add(e))
            }
            PatchDirection::Stage => entry.map(|e| e.oid) == file.old_oid,
            // A file deleted in stage is not in the index.
            PatchDirection::Unstage if file.change == Change::Deleted => entry.is_none(),
            PatchDirection::Unstage => entry.map(|e| e.oid) == file.new_oid,
        };
        if consistent {
            Ok(())
        } else {
            Err(EngineError::NotAllowed(msg!("engine-lines-index-changed")))
        }
    }

    /// True if the entry is that of an `intent-to-add` file: the empty blob in the repo's format
    /// (`git status` reports it with mode and id zero and Y = `A`).
    fn is_intent_to_add(&self, entry: &IndexEntry) -> bool {
        entry.oid.to_string() == self.empty_blob
    }

    /// The content of blob `oid` (`git cat-file blob`, no filters).
    fn blob(&self, oid: &Oid) -> Result<Vec<u8>, EngineError> {
        self.run(&["cat-file", "blob", &oid.to_string()])
    }

    /// Saves `content` as a blob (`hash-object -w --stdin --no-filters`: it is already in the form
    /// of the index, no `clean` filters or line-ending conversions) and returns its id.
    /// Writes only to the object database, not to the index.
    fn hash_blob(&self, content: &[u8]) -> Result<Oid, EngineError> {
        let args = ["hash-object", "-w", "--stdin", "--no-filters"];
        let out = self.run_input(&args, content, &[])?;
        Ok(Oid::from_hex(out.trim_ascii_end())?)
    }

    /// Writes the entry of `path` to the index: `Some((mode, id))` creates or replaces it,
    /// `None` removes it (`update-index -z --index-info`, path on stdin). It is the only step
    /// that writes the index. `--index-info` creates a new entry: the `--skip-worktree` and
    /// `--assume-unchanged` flags of the old one are lost and [`Self::restore_flags`] puts them back.
    fn write_index_entry(
        &self,
        path: &RepoPath,
        entry: Option<(u32, Oid)>,
    ) -> Result<(), EngineError> {
        let mut info = match &entry {
            Some((mode, oid)) => format!("{mode:o} {oid}\t").into_bytes(),
            // Mode 0: `update-index --index-info` removes the entry.
            None => format!("0 {}\t", Oid::from_bytes([0; Oid::LEN])).into_bytes(),
        };
        info.extend_from_slice(path.as_bytes());
        info.push(0);
        self.run_input(&["update-index", "-z", "--index-info"], &info, &[])?;
        Ok(())
    }

    /// After a write that recreates index entries (`update-index --index-info`,
    /// `restore --staged`), puts back the `--skip-worktree` and `--assume-unchanged` flags that the entries
    /// of `paths` had in `before` (letters of `ls-files -v`: `S`/`s` skip-worktree,
    /// lowercase assume-unchanged) and that they no longer have. Without it, a file outside the cone of
    /// a sparse checkout would look deleted from the worktree, and a later `git add -A` or `commit -a`
    /// would stage a deletion never made. Entries no longer in the
    /// index are skipped. If it fails, error: the entries written are correct anyway.
    fn restore_flags(&self, before: &IndexListing, paths: &[RepoPath]) -> Result<(), EngineError> {
        let wanted = |listing: &IndexListing| -> Vec<(RepoPath, bool, bool)> {
            listing
                .iter()
                .filter(|(p, _)| paths.iter().any(|w| path_matches(p, w)))
                .map(|(p, e)| {
                    (
                        p.clone(),
                        e.tag.eq_ignore_ascii_case(&b'S'),
                        e.tag.is_ascii_lowercase(),
                    )
                })
                .collect()
        };
        let flagged: Vec<_> = wanted(before)
            .into_iter()
            .filter(|(_, s, a)| *s || *a)
            .collect();
        if flagged.is_empty() {
            return Ok(());
        }
        let now = self.index_listing()?;
        let mut skip = Vec::new();
        let mut assume = Vec::new();
        for (path, s, a) in flagged {
            let Some(e) = now.get(&path) else { continue };
            if s && !e.tag.eq_ignore_ascii_case(&b'S') {
                skip.extend_from_slice(path.as_bytes());
                skip.push(0);
            }
            if a && !e.tag.is_ascii_lowercase() {
                assume.extend_from_slice(path.as_bytes());
                assume.push(0);
            }
        }
        for (flag, list) in [("--skip-worktree", skip), ("--assume-unchanged", assume)] {
            if list.is_empty() {
                continue;
            }
            let args = ["update-index", flag, "-z", "--stdin"];
            self.run_input(&args, &list, &[])
                .map_err(|e| EngineError::GitWithReason {
                    command: "update-index".to_owned(),
                    reason: msg!("engine-index-flags-not-restored", flag = flag, error = e),
                })?;
        }
        Ok(())
    }

    /// `git <args>` with `input` on stdin and the variables `env`; stdout, or git's error.
    fn run_input(
        &self,
        args: &[&str],
        input: &[u8],
        env: &[(&str, &OsStr)],
    ) -> Result<Vec<u8>, EngineError> {
        let out = self
            .git
            .output_with_input_env(Some(&self.workdir), args, input, env)?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(failure(args, &out))
        }
    }

    /// Runs `git <args>` with `paths` read from stdin (`--pathspec-from-file=-
    /// --pathspec-file-nul`, taken literally by `--literal-pathspecs`): no limit on the
    /// length of the command line and no path that git could read as an option.
    fn output_with_paths(
        &self,
        args: &[&str],
        paths: &[RepoPath],
    ) -> Result<std::process::Output, EngineError> {
        debug_assert!(
            !paths.is_empty(),
            "without paths git would work on the whole repo"
        );
        let mut full = vec!["--literal-pathspecs"];
        full.extend_from_slice(args);
        full.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        self.git
            .output_with_input(Some(&self.workdir), &full, &input)
    }

    /// Operation in progress, from the files git leaves in its directory (like `git status`).
    ///
    /// `rebase-apply/applying` indicates `git am` ([`Operation::Am`]). A multi-commit cherry-pick or revert
    /// stopped between one commit and the next (after `git commit` of a resolution, or with
    /// an empty commit) leaves `sequencer/` without `CHERRY_PICK_HEAD`/`REVERT_HEAD`: it is still in
    /// progress, and the kind is read from the first line of `sequencer/todo` (`revert ...` →
    /// [`Operation::Revert`], otherwise [`Operation::CherryPick`]).
    fn operation(&self) -> Option<Operation> {
        let has = |name: &str| self.git_dir.join(name).exists();
        if has("MERGE_HEAD") {
            Some(Operation::Merge)
        } else if has("rebase-apply/applying") {
            Some(Operation::Am)
        } else if has("rebase-merge") || has("rebase-apply") {
            Some(Operation::Rebase)
        } else if has("CHERRY_PICK_HEAD") {
            Some(Operation::CherryPick)
        } else if has("REVERT_HEAD") {
            Some(Operation::Revert)
        } else if has("sequencer") {
            Some(sequencer_operation(
                &std::fs::read(self.git_dir.join("sequencer/todo")).unwrap_or_default(),
            ))
        } else if has("BISECT_LOG") {
            Some(Operation::Bisect)
        } else {
            None
        }
    }
}

/// The kind of a stopped sequencer from the content of `sequencer/todo`: the first line that is not
/// a comment says `revert` or `pick`; with no lines (or unreadable) cherry-pick.
fn sequencer_operation(todo: &[u8]) -> Operation {
    let first = todo
        .split(|&b| b == b'\n')
        .map(<[u8]>::trim_ascii)
        .find(|l| !l.is_empty() && !l.starts_with(b"#"));
    match first {
        Some(line) if line.starts_with(b"revert ") || line.starts_with(b"r ") => Operation::Revert,
        _ => Operation::CherryPick,
    }
}

impl CliBackend {
    /// The revisions `git log` starts from for `query` (`repo.log`): `--branches --remotes
    /// --tags` with `all`, then `--end-of-options` and the ids (HEAD, `rev`, the stash bases);
    /// `None` if the log is empty (HEAD without commits and no `all`). Unaffected by
    /// `skip`/`limit`.
    pub(crate) fn log_range(&self, query: &LogQuery) -> Result<Option<Vec<String>>, EngineError> {
        query.check()?;
        let start = match &query.rev {
            Some(rev) => Some(
                self.resolve(rev)?
                    .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?,
            ),
            // Branch with no commits: empty log (with `all` the other refs remain).
            None => self.resolve("HEAD")?,
        };
        if start.is_none() && !query.all {
            return Ok(None);
        }
        let mut range = Vec::new();
        if query.all {
            // Only these namespaces: `refs/stash`, the backups in `refs/project/` and notes stay
            // out. With no refs the log is empty (git does not fall back to HEAD).
            range.extend(["--branches", "--remotes", "--tags"].map(String::from));
        }
        // HEAD as an id, so a detached HEAD also appears with `all`.
        range.push("--end-of-options".to_owned());
        range.extend(start.map(|oid| oid.to_string()));
        // With `stashes` also the stash bases (not the stashes themselves).
        if query.stashes {
            range.extend(self.stash_entries()?.iter().map(|s| s.base.to_string()));
        }
        Ok(Some(range))
    }
}

impl GitRead for CliBackend {
    fn head(&self) -> Result<Head, EngineError> {
        let args = ["symbolic-ref", "--quiet", "HEAD"];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            let full = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
            let full = String::from_utf8_lossy(full);
            let name = branch_short(&full).to_owned();
            return Ok(Head::Branch {
                name,
                target: self.resolve("HEAD")?,
            });
        }
        // Code 1 with no messages: HEAD is not symbolic, i.e. detached.
        if out.status.code() != Some(1) || !out.stderr.is_empty() {
            return Err(failure(&args, &out));
        }
        match self.resolve("HEAD")? {
            Some(oid) => Ok(Head::Detached(oid)),
            None => Err(EngineError::UnknownRev("HEAD".to_owned())),
        }
    }

    fn refs(&self) -> Result<Vec<RefInfo>, EngineError> {
        let mut args = vec!["for-each-ref"];
        args.extend_from_slice(REFS_ARGS);
        Ok(parse_refs(&self.run(&args)?)?)
    }

    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>, EngineError> {
        let Some(range) = self.log_range(query)? else {
            return Ok(Vec::new());
        };
        if query.limit == Some(0) {
            return Ok(Vec::new());
        }
        let skip = format!("--skip={}", query.skip);
        let max_count = query.limit.map(|n| format!("--max-count={n}"));

        let mut args = vec!["log"];
        args.extend_from_slice(LOG_ARGS);
        // Children before parents, then by committer date, like `MemoryBackend`; it also holds
        // with multiple starting points.
        args.extend(["--date-order", &skip]);
        args.extend(max_count.as_deref());
        args.extend(range.iter().map(String::as_str));
        args.push("--");
        Ok(parse_log(&self.run(&args)?)?)
    }

    fn search(&self, query: &SearchQuery, job: &JobContext) -> Result<Vec<Oid>, EngineError> {
        self.search_log(query, job)
    }

    fn status(&self) -> Result<Status, EngineError> {
        // A read must not take the index lock to update it.
        let mut args = vec!["--no-optional-locks", "status"];
        args.extend_from_slice(STATUS_ARGS);
        let mut status = parse_status(&self.run(&args)?)?;
        status.operation = self.operation();
        Ok(status)
    }

    fn show_commit(&self, rev: &str) -> Result<CommitDetails, EngineError> {
        let id = self
            .resolve(rev)?
            .ok_or_else(|| EngineError::UnknownRev(rev.to_owned()))?;
        let hex = id.to_string();

        let mut args = vec!["log"];
        args.extend_from_slice(LOG_ARGS);
        args.extend(["--max-count=1", "--end-of-options", &hex, "--"]);
        let commit = parse_log(&self.run(&args)?)?
            .into_iter()
            .next()
            .ok_or_else(|| EngineError::GitWithReason {
                command: "log".to_owned(),
                reason: msg!("engine-git-no-commit", hex = hex),
            })?;

        // Comparison with the first parent, also for merges (without options `diff-tree` would
        // show nothing for a merge); a root commit is compared with the empty tree.
        let compared_to = commit.parents.first().copied();
        let parent = compared_to.map(|p| p.to_string());
        let mut args = vec!["diff-tree"];
        args.extend_from_slice(DIFF_TREE_ARGS);
        match &parent {
            Some(parent) => args.extend(["--end-of-options", parent, &hex]),
            None => args.extend(["--root", "--end-of-options", &hex]),
        }
        args.push("--");
        let files = parse_diff_tree(&self.run(&args)?)?;
        Ok(CommitDetails {
            commit,
            compared_to,
            files,
        })
    }

    fn diff(&self, spec: &DiffSpec) -> Result<Diff, EngineError> {
        let (parent, id);
        let compare: Vec<&str> = match spec {
            DiffSpec::Worktree { .. } => Vec::new(),
            // On a branch with no commits git compares the index with the empty tree.
            DiffSpec::Index { .. } => vec!["--cached"],
            DiffSpec::Commit { rev, .. } => {
                id = self
                    .resolve(rev)?
                    .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?
                    .to_string();
                // First parent also for merges, like `show_commit`.
                parent = match self.first_parent(&id)? {
                    Some(p) => p.to_string(),
                    None => self.empty_tree.to_owned(),
                };
                vec!["--end-of-options", &parent, &id]
            }
        };
        let Some(path) = spec.path() else {
            return Ok(parse_diff(&self.run_diff(DIFF_ARGS, &compare, &[])?)?);
        };
        let paths = self.diff_paths(&compare, path)?;
        let paths: Vec<&RepoPath> = paths.iter().collect();
        let mut diff = parse_diff(&self.run_diff(DIFF_ARGS, &compare, &paths)?)?;
        // A literal path also includes files under a directory with that name (e.g. if
        // `path` was a file and is now a directory): only the file with that exact path.
        diff.files
            .retain(|f| f.new_path == *path || f.old_path == *path);
        Ok(diff)
    }

    fn blame(&self, query: &BlameQuery, job: &JobContext) -> Result<FileBlame, EngineError> {
        self.blame_file(query, job)
    }

    fn file_log(
        &self,
        query: &FileLogQuery,
        job: &JobContext,
    ) -> Result<Vec<FileLogEntry>, EngineError> {
        self.log_file(query, job)
    }

    fn root(&self) -> Option<String> {
        self.workdir.to_str().map(str::to_owned)
    }

    fn unmerged_count(&self, tip: &Oid, into: &[Oid]) -> Result<usize, EngineError> {
        self.count_unmerged(tip, into)
    }

    fn config_values(&self, key: &str) -> Result<Vec<String>, EngineError> {
        self.config_all(key)
    }
}

impl GitIndex for CliBackend {
    fn stage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError> {
        if paths.is_empty() {
            // `git add` with no paths adds nothing, but better not to ask for it at all.
            return Ok(None);
        }
        let status = self.status_for_write()?;
        let index = self.index_listing()?;
        // A conflicted file that is staged is resolved: only if requested by name, not because
        // it is inside a requested directory.
        let in_folder: Vec<&RepoPath> = status
            .entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| &e.path)
            .filter(|u| !paths.contains(u) && paths.iter().any(|p| path_matches(u, p)))
            .collect();
        if !in_folder.is_empty() {
            return Err(EngineError::unmerged_in_folder(&in_folder));
        }
        let paths = self.without_staged_deletions(&status, paths);
        if paths.is_empty() {
            return Ok(None);
        }
        // `--no-ignore-errors`: an unreadable file stops everything (the `add.ignoreErrors` config
        // would make it continue with the others).
        let add = ["add", "--no-ignore-errors"];
        // First a dry run: with an ignored file `git add` adds the others and then fails,
        // with a nonexistent path it fails before adding. The dry run does the
        // same checks without touching the index, so either everything goes through or nothing changes.
        let dry = [&add[..], &["--dry-run"]].concat();
        let out = self.output_with_paths(&dry, &paths)?;
        if !out.status.success() {
            let stderr = stderr_text(&out);
            return Err(match ignored_paths(&stderr) {
                Some(ignored) => EngineError::ignored(&ignored),
                None => failure(&dry, &out),
            });
        }
        // The staged versions that would remain only in an unreachable blob: first a
        // backup; if it fails, the index is not touched.
        let backup = self.backup("index.stage", &status, &index, &paths)?;
        self.check_index_unchanged(&status, &index, &paths)?;
        let out = self.output_with_paths(&add, &paths)?;
        if !out.status.success() {
            return Err(failure(&add, &out));
        }
        Ok(backup)
    }

    fn unstage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError> {
        if paths.is_empty() {
            return Ok(None);
        }
        let status = self.status_for_write()?;
        let index = self.index_listing()?;
        // `restore --staged` on a conflicted file would silently remove the conflict
        // stages (the base, ours and theirs versions).
        let conflicts: Vec<&RepoPath> = status
            .entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| &e.path)
            .filter(|u| paths.iter().any(|p| path_matches(u, p)))
            .collect();
        if !conflicts.is_empty() {
            return Err(EngineError::unmerged(&conflicts));
        }
        // The index goes back as in HEAD; with no commit as the empty tree, i.e. the files go back
        // untracked (`restore` without `--source` would fail: HEAD does not resolve).
        // `--no-overlay` (already the default of `restore`): files that are not in HEAD leave
        // the index. Without `--worktree` the worktree is not touched.
        let source = format!("--source={}", self.index_source()?);
        let backup = self.backup("index.unstage", &status, &index, paths)?;
        self.check_index_unchanged(&status, &index, paths)?;
        let args = ["restore", "--staged", "--no-overlay", &source];
        let out = self.output_with_paths(&args, paths)?;
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        // `restore --staged` recreates the entries without `--skip-worktree`/`--assume-unchanged`.
        self.restore_flags(&index, paths)?;
        Ok(backup)
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
        self.restore_discarded(request)
    }

    fn apply_lines(&mut self, selection: &LineSelection) -> Result<Option<String>, EngineError> {
        let operation = selection.command_id();
        let status = self.status_for_write()?;
        let index = self.index_listing()?;
        // The diff is read after the index: if in the meantime another git changed the entry,
        // `check_diff_matches_index` notices.
        let diff = self.diff(&selection.diff_spec())?;
        let prepared = line_patch(&diff, selection)?;
        self.check_diff_matches_index(&index, &prepared, selection.direction)?;

        // The new content is computed here, with the strict applier, on the blob of the entry
        // just checked: each hunk only at its position, context byte for byte. `git apply` is
        // not used, since it looks for the preimage elsewhere too (with little context it could
        // put the lines in the wrong place and still succeed).
        let target = prepared.target().clone();
        let entry = index.get(&target);
        let base = match entry {
            Some(e) if !(prepared.patch.change == Change::Added && self.is_intent_to_add(e)) => {
                Some(self.blob(&e.oid)?)
            }
            _ => None,
        };
        let content = apply_patch(base.as_deref(), &prepared.patch).map_err(|e| {
            EngineError::NotAllowed(msg!(
                "engine-lines-patch-failed",
                target = target,
                error = e
            ))
        })?;
        // The mode stays that of the index; for a file that was not in the index (or was only
        // `intent-to-add`) that of the new side of the patch.
        let mode = match entry {
            Some(e) if base.is_some() => e.mode,
            _ => prepared
                .patch
                .new_mode
                .or(entry.map(|e| e.mode))
                .unwrap_or(0o100644),
        };

        self.check_index_unlocked()?;
        // The blob is saved before the backup and the recheck: between the recheck and the
        // write only `update-index` remains. With the git CLI there is no real
        // compare-and-swap on the index: the instant remains between that recheck and the lock
        // taken by `update-index`.
        let written = match &content {
            Some(c) => Some((mode, self.hash_blob(c)?)),
            None => None,
        };
        let touched = prepared.touched(selection);
        let backup = self.backup(operation, &status, &index, &touched)?;
        self.check_index_unchanged(&status, &index, &touched)?;
        self.write_index_entry(&target, written)?;
        self.restore_flags(&index, std::slice::from_ref(&target))?;

        // The entry in the index is the one written (another git could have changed it right
        // after: in that case it is reported, the backup stays).
        let now = self.index_listing()?;
        if now.get(&target).map(|e| (e.mode, e.oid)) != written {
            let reason = match &backup {
                Some(b) => msg!(
                    "engine-lines-index-unexpected-backup",
                    target = target,
                    backup = b
                ),
                None => msg!("engine-lines-index-unexpected", target = target),
            };
            return Err(EngineError::GitWithReason {
                command: "update-index".to_owned(),
                reason,
            });
        }
        Ok(backup)
    }
}

impl GitRefs for CliBackend {
    fn restore_refs(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError> {
        self.restore(request)
    }

    fn reset_branch(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError> {
        self.reset_keep(request)
    }

    fn delete_tag(&mut self, name: &str, expected: &Oid) -> Result<String, EngineError> {
        self.delete_tag_ref(name, expected)
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

    fn checkout(&mut self, target: &HeadState) -> Result<RestoreOutcome, EngineError> {
        self.switch_to(target)
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

    fn reset(
        &mut self,
        request: &project_engine::ResetRequest,
    ) -> Result<project_engine::ResetOutcome, EngineError> {
        self.reset_head(request)
    }
}

impl GitRemotes for CliBackend {
    fn remotes(&self) -> Result<Vec<String>, EngineError> {
        self.remote_names()
    }

    fn fetch(
        &mut self,
        request: &FetchRequest,
        job: &JobContext,
    ) -> Result<FetchOutcome, EngineError> {
        self.fetch_remote(request, job)
    }

    fn upstream(&self, branch: &str) -> Result<Option<UpstreamConfig>, EngineError> {
        self.upstream_of(branch)
    }

    fn pull_settings(&self, branch: &str) -> Result<PullSettings, EngineError> {
        self.read_pull_settings(branch)
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
        self.snapshot_remote(name)
    }

    fn create_remote(&mut self, name: &str, url: &str) -> Result<(), EngineError> {
        self.add_remote(name, url)
    }

    fn remove_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<Message> {
        self.drop_remote_config(snapshot)
    }

    fn write_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<Message> {
        self.put_remote_config(snapshot)
    }

    fn remote_config_elsewhere(&self, name: &str) -> Result<Vec<String>, EngineError> {
        self.remote_elsewhere(name)
    }
}

impl GitStash for CliBackend {
    fn stashes(&self) -> Result<Vec<StashEntry>, EngineError> {
        self.stash_entries()
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

impl GitOperations for CliBackend {
    fn merge(&mut self, request: &MergeRequest) -> Result<MergeOutcome, EngineError> {
        self.merge_into_head(request)
    }

    fn merge_abort(&mut self) -> Result<MergeAborted, EngineError> {
        self.abort_merge()
    }

    fn conflict_versions(&self, path: &RepoPath) -> Result<ConflictVersions, EngineError> {
        self.conflict_files(path)
    }

    fn resolve_conflict(
        &mut self,
        request: &ConflictResolution,
    ) -> Result<ConflictResolved, EngineError> {
        self.resolve_file(request)
    }

    fn operation_state(&self) -> Result<OperationState, EngineError> {
        Ok(self.read_operation_state())
    }

    fn pick(&mut self, request: &PickRequest) -> Result<MergeOutcome, EngineError> {
        self.pick_onto_head(request)
    }

    fn pick_abort(&mut self, kind: PickKind) -> Result<MergeAborted, EngineError> {
        self.abort_pick(kind)
    }

    fn rebase(&mut self, request: &RebaseRequest) -> Result<MergeOutcome, EngineError> {
        self.rebase_onto_base(request)
    }

    fn rebase_continue(&mut self) -> Result<MergeOutcome, EngineError> {
        self.rebase_go_on()
    }

    fn rebase_abort(&mut self) -> Result<MergeAborted, EngineError> {
        self.abort_rebase()
    }
}

impl GitMaintenance for CliBackend {
    fn commit_graph(&self) -> Result<CommitGraphState, EngineError> {
        self.read_commit_graph_state()
    }

    fn write_commit_graph(&mut self, job: &JobContext) -> Result<(), EngineError> {
        self.write_reachable_commit_graph(job)
    }
}

/// The paths listed by `git add` when it refuses ignored files (with `LC_ALL=C`):
///
/// ```text
/// The following paths are ignored by one of your .gitignore files:
/// <path>
/// hint: Use -f if you really want to add them.
/// ```
///
/// `None` if stderr does not contain that message.
fn ignored_paths(stderr: &str) -> Option<Vec<String>> {
    const HEADER: &str = "The following paths are ignored by one of your .gitignore files:";
    let start = stderr.find(HEADER)? + HEADER.len();
    let paths: Vec<String> = stderr[start..]
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .take_while(|l| !l.starts_with("hint:") && !l.starts_with("fatal:"))
        .map(str::to_owned)
        .collect();
    Some(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_paths_from_git_add() {
        let stderr = "The following paths are ignored by one of your .gitignore files:\n\
                      debug.log\n\
                      dir/x.log\n\
                      hint: Use -f if you really want to add them.\n\
                      hint: Turn this message off by running";
        assert_eq!(
            ignored_paths(stderr),
            Some(vec!["debug.log".to_owned(), "dir/x.log".to_owned()])
        );
        assert_eq!(
            ignored_paths("fatal: pathspec 'x' did not match any files"),
            None
        );
    }

    #[test]
    fn sequencer_kind_from_todo() {
        assert_eq!(
            sequencer_operation(b"revert 1234567 x\npick 89abcde y\n"),
            Operation::Revert
        );
        assert_eq!(
            sequencer_operation(b"# c\npick 1234567 x\n"),
            Operation::CherryPick
        );
        assert_eq!(sequencer_operation(b""), Operation::CherryPick);
    }

    #[test]
    fn empty_blob_per_object_format() {
        // `git hash-object -t blob /dev/null`, the entry of an `intent-to-add` file.
        assert_eq!(
            empty_blob(b"sha1"),
            Some("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391")
        );
        assert_eq!(empty_blob(b"sha256").map(str::len), Some(64));
        assert_eq!(empty_blob(b"md5"), None);
    }

    #[test]
    fn empty_tree_per_object_format() {
        assert_eq!(
            empty_tree(b"sha1"),
            Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904")
        );
        assert_eq!(empty_tree(b"sha256").map(str::len), Some(64));
        assert_eq!(empty_tree(b"md5"), None);
    }
}
