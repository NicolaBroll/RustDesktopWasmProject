//! `worktree.ignore` and its undo/redo on the [`CliBackend`] (steps and guarantees in
//! `impl-docs/git/ignore.md`).
//!
//! Ignore, in this order, and nothing is written if a step fails:
//!
//! 1. the file: `<root>/.gitignore`, or `git rev-parse --git-path info/exclude` (linked
//!    worktrees, `GIT_DIR`, `GIT_COMMON_DIR`); a `.gitignore` with a conflict is refused;
//! 2. `lstat` of its folder (a real folder; for `info/` also missing) and of the file (missing
//!    or a regular file: a link, a folder or a special file is refused), then its bytes;
//! 3. [`append_ignore_patterns`]: if every pattern is already there, nothing more happens;
//! 4. the untracked files now (`ls-files -z --others --exclude-standard`) and the tracked files
//!    the new patterns alone match (`ls-files -z --cached --ignored --exclude=<pattern>...`);
//! 5. backup: the bytes before and after as blobs (`hash-object -w --no-filters`), a commit with
//!    the bytes written and, as its child, a commit with the bytes before (none if the file did
//!    not exist), held by a new ref `refs/project/backup/ignore-...` and verified with
//!    `ls-tree`: both blobs of the journal entry stay reachable;
//! 6. **last check**: the file still has the bytes read in step 2 (or is still missing);
//! 7. atomic write ([`atomic`](super::atomic)): `info/` created if missing; an existing file
//!    replaced (temporary file and rename, permissions kept), a new one linked in place
//!    (never overwriting);
//! 8. the file reread (a warning if it does not have the bytes written), the untracked files
//!    again: those gone are the ones that became ignored.
//!
//! Undo/redo: the file must have exactly the expected bytes (or be missing), then the other
//! bytes are written the same way (or the file removed). Afterwards it is reread: if it does not
//! have the bytes written, a warning.

use super::CliBackend;
use super::atomic::{create_file, replace_file};
use crate::command::{os_from_bytes, path_from_output};
use project_domain::{IgnorePattern, Oid, RepoPath, append_ignore_patterns};
use project_engine::{
    EngineError, IgnoreFile, IgnoreOutcome, IgnoreRecord, IgnoreRequest, IgnoreRestore, Message,
    RestoreOutcome, StagedVersion,
};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Reflog and first line of the backup commits: fixed, no user data.
const BACKUP_MESSAGE: &str = "project: backup of the ignore file before worktree.ignore";

impl CliBackend {
    /// [`GitIndex::ignore`](project_engine::GitIndex::ignore).
    pub(super) fn ignore_patterns(
        &mut self,
        request: &IgnoreRequest,
    ) -> Result<IgnoreOutcome, EngineError> {
        let file = request.file;
        if file == IgnoreFile::Gitignore {
            let unmerged = self.run(&["ls-files", "-z", "--unmerged", "--", ".gitignore"])?;
            if !unmerged.is_empty() {
                return Err(EngineError::NotAllowed(msg!("engine-ignore-conflicted")));
            }
        }
        let full = self.ignore_file_path(file)?;
        let old = read_ignore_file(file, &full)?;
        if old.is_none() && file == IgnoreFile::Gitignore {
            // Tracked but not on disk (`--skip-worktree`, sparse checkout): git reads its rules
            // from the index, and a new file with only the new patterns would replace them all.
            let tracked = self.run(&["ls-files", "-z", "--cached", "--", ".gitignore"])?;
            if !tracked.is_empty() {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-ignore-not-checked-out"
                )));
            }
        }
        let append = append_ignore_patterns(old.as_deref(), &request.patterns);
        let mut outcome = IgnoreOutcome {
            file,
            added: append.added,
            already: append.already,
            ignored: Vec::new(),
            tracked: Vec::new(),
            created: old.is_none(),
            record: None,
            backup: None,
            warnings: Vec::new(),
        };
        if outcome.added.is_empty() {
            outcome.created = false;
            return Ok(outcome);
        }
        let untracked_before = self.untracked_files()?;
        outcome.tracked = self.tracked_matching(&outcome.added)?;

        let before = old.as_deref().map(|b| self.hash_blob(b)).transpose()?;
        let after = self.hash_blob(&append.content)?;
        let backup = self.ignore_backup(file, before, after)?;

        // Last check before writing: the file is still the one read.
        if read_ignore_file(file, &full).ok() != Some(old.clone()) {
            return Err(EngineError::NotAllowed(msg!(
                "engine-ignore-changed",
                file = file.label()
            )));
        }
        let note = backup.clone();
        write_ignore_file(file, &full, old.is_some(), Some(&append.content), &note)?;
        outcome.warnings = not_written("worktree.ignore", file, &full, &append.content, &note);
        match self.untracked_files() {
            Ok(after_untracked) => {
                outcome.ignored = untracked_before
                    .difference(&after_untracked)
                    .cloned()
                    .collect();
            }
            // The file is written: the command must not fail now.
            Err(e) => outcome
                .warnings
                .push(msg!("engine-other", text = e.to_string())),
        }
        outcome.record = Some(IgnoreRecord {
            file,
            before,
            after,
        });
        outcome.backup = Some(backup);
        Ok(outcome)
    }

    /// [`GitIndex::restore_ignore`](project_engine::GitIndex::restore_ignore).
    pub(super) fn restore_ignore_file(
        &mut self,
        request: &IgnoreRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        let file = request.record.file;
        let full = self.ignore_file_path(file)?;
        let (expected, wanted) = request.blobs();
        let expected = expected.map(|oid| self.blob(&oid)).transpose()?;
        let content = wanted.map(|oid| self.blob(&oid)).transpose()?;
        let now = read_ignore_file(file, &full).map_err(|_| request.conflict())?;
        if now != expected {
            return Err(request.conflict());
        }
        // The bytes overwritten stay in the object database as the blob expected.
        let note = match request.blobs().0 {
            Some(oid) => format!("blob {oid}"),
            None => "-".to_owned(),
        };
        write_ignore_file(file, &full, now.is_some(), content.as_deref(), &note)?;
        let warnings = match &content {
            Some(bytes) => not_written(request.command, file, &full, bytes, &note),
            None if std::fs::symlink_metadata(&full).is_ok() => vec![msg!(
                "engine-ignore-not-as-expected",
                command = request.command,
                file = file.label(),
                backup = note
            )],
            None => Vec::new(),
        };
        Ok(RestoreOutcome {
            backups: Vec::new(),
            warnings,
        })
    }

    /// Where the file is: `<root>/.gitignore` or `git rev-parse --git-path info/exclude`
    /// (relative to the root if git gives a relative path).
    fn ignore_file_path(&self, file: IgnoreFile) -> Result<PathBuf, EngineError> {
        match file {
            IgnoreFile::Gitignore => Ok(self.workdir.join(".gitignore")),
            IgnoreFile::Exclude => {
                let out = self.run(&["rev-parse", "--git-path", "info/exclude"])?;
                let path = path_from_output(&out);
                Ok(if path.is_absolute() {
                    path
                } else {
                    self.workdir.join(path)
                })
            }
        }
    }

    /// The untracked, not ignored files now (`ls-files -z --others --exclude-standard`, which
    /// reads `.gitignore`, `info/exclude` and `core.excludesFile`).
    fn untracked_files(&self) -> Result<BTreeSet<RepoPath>, EngineError> {
        let out = self.run(&["ls-files", "-z", "--others", "--exclude-standard"])?;
        Ok(out
            .split(|&b| b == 0)
            .filter(|r| !r.is_empty())
            .map(RepoPath::from_bytes)
            .collect())
    }

    /// The tracked files that `patterns` alone (relative to the root, as in `info/exclude`)
    /// match: `ls-files -z --cached --ignored --exclude=<pattern>...`, in path order. Each
    /// pattern is one argument `--exclude=<bytes>`: it cannot become an option.
    fn tracked_matching(&self, patterns: &[IgnorePattern]) -> Result<Vec<RepoPath>, EngineError> {
        let mut args: Vec<OsString> = ["ls-files", "-z", "--cached", "--ignored"]
            .iter()
            .map(OsString::from)
            .collect();
        for pattern in patterns {
            let mut arg = OsString::from("--exclude=");
            arg.push(os_from_bytes(pattern.as_bytes()));
            args.push(arg);
        }
        let args: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
        let out = self.git.output_os(Some(&self.workdir), &args)?;
        if !out.status.success() {
            return Err(crate::command::failure(&["ls-files"], &out));
        }
        let paths: BTreeSet<RepoPath> = out
            .stdout
            .split(|&b| b == 0)
            .filter(|r| !r.is_empty())
            .map(RepoPath::from_bytes)
            .collect();
        Ok(paths.into_iter().collect())
    }

    /// The backup of a `worktree.ignore`: a commit with the bytes written (`after`) and, as its
    /// child, a commit with the bytes before (`before`; an empty tree if the file did not
    /// exist), held by a new ref `refs/project/backup/ignore-...`, verified with `ls-tree`.
    fn ignore_backup(
        &self,
        file: IgnoreFile,
        before: Option<Oid>,
        after: Oid,
    ) -> Result<String, EngineError> {
        let path = file.backup_path();
        let version = |oid: Oid| StagedVersion {
            path: path.clone(),
            mode: 0o100644,
            oid,
        };
        let label = file.label();
        let written_tree = self.backup_tree(&[version(after)])?;
        let message = format!(
            "{BACKUP_MESSAGE}\n\nThe bytes worktree.ignore writes to {label} (the child commit \
             has the bytes before).\n"
        );
        let written = self.backup_commit_of(&written_tree, &message, None)?;
        let before_tree = match before {
            Some(oid) => self.backup_tree(&[version(oid)])?,
            None => self.empty_tree.to_owned(),
        };
        let message = format!(
            "{BACKUP_MESSAGE}\n\nThe exact bytes of {label} before worktree.ignore (`git show \
             <commit>:{path}`; no file: it did not exist). The parent has the bytes written.\n"
        );
        let commit = self.backup_commit_of(&before_tree, &message, Some(&written))?;
        let backup = self.create_backup_ref("ignore", &commit.to_string(), BACKUP_MESSAGE)?;
        let paths = std::slice::from_ref(&path);
        let ok = self.ls_tree(&commit, paths)?.get(&path).map(|e| e.1) == before
            && self.ls_tree(&written, paths)?.get(&path).map(|e| e.1) == Some(after);
        if !ok {
            return Err(EngineError::NotAllowed(msg!(
                "engine-ignore-backup-incomplete",
                backup = backup,
                file = label
            )));
        }
        Ok(backup)
    }

    /// A commit of `tree` with `parent` (if any), the fixed identity, never signed.
    fn backup_commit_of(
        &self,
        tree: &str,
        message: &str,
        parent: Option<&Oid>,
    ) -> Result<Oid, EngineError> {
        let parent = parent.map(Oid::to_string);
        let mut args = vec!["commit-tree", "--no-gpg-sign"];
        if let Some(parent) = &parent {
            args.extend(["-p", parent]);
        }
        args.extend(["-F", "-", tree]);
        let identity: Vec<(&str, &OsStr)> = super::BACKUP_IDENTITY
            .iter()
            .map(|(k, v)| (*k, OsStr::new(v)))
            .collect();
        let commit = self.run_input(&args, message.as_bytes(), &identity)?;
        Ok(Oid::from_hex(commit.trim_ascii_end())?)
    }
}

/// The bytes of the ignore file at `full` (`None`: it does not exist). Its folder must be a real
/// folder (or, for `info/`, missing: then so is the file); the file must be a regular file.
fn read_ignore_file(file: IgnoreFile, full: &Path) -> Result<Option<Vec<u8>>, EngineError> {
    let label = file.label();
    if let Some(dir) = full.parent() {
        match std::fs::symlink_metadata(dir) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && file == IgnoreFile::Exclude => {
                return Ok(None);
            }
            _ => {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-ignore-dir-not-real",
                    file = label
                )));
            }
        }
    }
    let unreadable = |e: std::io::Error| EngineError::GitWithReason {
        command: "read".to_owned(),
        reason: msg!("engine-ignore-unreadable", file = label, error = e),
    };
    let meta = match std::fs::symlink_metadata(full) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unreadable(e)),
    };
    if !meta.is_file() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-ignore-not-a-file",
            file = label
        )));
    }
    std::fs::read(full).map(Some).map_err(unreadable)
}

/// Writes `content` to the ignore file (`exists`: it is there now and is replaced; otherwise it
/// is created, with `info/` if missing), or removes it (`None`). The error names `backup`.
fn write_ignore_file(
    file: IgnoreFile,
    full: &Path,
    exists: bool,
    content: Option<&[u8]>,
    backup: &str,
) -> Result<(), EngineError> {
    let written = match content {
        None => std::fs::remove_file(full),
        Some(bytes) if exists => replace_file(full, bytes),
        Some(bytes) => {
            let created_dir = match full.parent() {
                Some(dir) if file == IgnoreFile::Exclude => create_real_dir(dir),
                _ => Ok(()),
            };
            created_dir.and_then(|()| create_file(full, bytes))
        }
    };
    written.map_err(|e| EngineError::GitWithReason {
        command: "write".to_owned(),
        reason: msg!(
            "engine-ignore-write-failed",
            file = file.label(),
            error = e,
            backup = backup
        ),
    })
}

/// Creates the folder `dir` if missing (only that level: its parent, the git directory, exists)
/// and checks that it is a real folder, not a link.
fn create_real_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(std::io::ErrorKind::NotADirectory.into());
    }
    Ok(())
}

/// A warning if the file reread does not have `content` (another program wrote it right
/// after?).
fn not_written(
    command: &str,
    file: IgnoreFile,
    full: &Path,
    content: &[u8],
    backup: &str,
) -> Vec<Message> {
    match read_ignore_file(file, full) {
        Ok(Some(bytes)) if bytes == content => Vec::new(),
        _ => vec![msg!(
            "engine-ignore-not-as-expected",
            command = command,
            file = file.label(),
            backup = backup
        )],
    }
}
