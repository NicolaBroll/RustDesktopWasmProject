//! `worktree.discard` and its undo/redo on the [`CliBackend`] (steps and guarantees
//! in `impl-docs/git/discard.md`).
//!
//! Discard, in this order, and nothing is written to the worktree if a step fails:
//!
//! 1. status without `core.fsmonitor`, [`discard_plan`] and comparison with the confirmed plan;
//!    the index (`ls-files -s`) must have the entries of the status;
//! 2. every folder in the path of every file is a real folder and the file is not a folder
//!    (an untracked, ignored or link in its place would be deleted by `git restore`);
//! 3. `index.lock` → [`EngineError::IndexLocked`] right away;
//! 4. `git stash create` (without fsmonitor, fixed backup identity);
//! 5. the files whose bytes are not the ones `git restore` would recreate from the commit (filters,
//!    `core.autocrlf=input`) are saved without filters in a child commit (`raw`);
//! 6. the ref `refs/project/backup/discard-...` (on `raw` if there is one, otherwise on the stash);
//! 7. recheck of index and folders, and **last verification** that the backup has exactly
//!    the current worktree (mode with `git diff <stash>`, bytes as in step 5);
//! 8. `git restore --worktree --no-overlay` of only the files of the plan (paths on stdin).
//!
//! Undo: HEAD (checked by the journal), recorded index entry and worktree equal to
//! the index; then `git restore --source=<stash>` and the `raw` files rewritten with their bytes.
//! Redo: index entry and worktree equal to the backup, then `restore --worktree` again. After
//! the write it is re-read: a file not as expected goes in `warnings`.

use super::CliBackend;
use crate::command::{failure, os_from_bytes, stderr_text};
use project_domain::{EntryKind, Oid, RepoPath};
use project_engine::{
    DiscardOutcome, DiscardedFile, EngineError, RestoreOutcome, WorktreeRestore, check_same_plan,
    discard_plan,
};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The bytes of a system path (the target of a link), as git stores them.
#[cfg(unix)]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    s.to_string_lossy().replace('\\', "/").into_bytes()
}

/// Commit message of `git stash create`: fixed, no user data.
const STASH_MESSAGE: &str = "project: backup of the worktree before worktree.discard";

impl CliBackend {
    /// [`GitBackend::discard`](project_engine::GitBackend::discard).
    pub(super) fn discard_paths(
        &mut self,
        paths: &[RepoPath],
        expected: &[DiscardedFile],
    ) -> Result<DiscardOutcome, EngineError> {
        let status = self.status_for_write()?;
        let files = discard_plan(&status, paths)?;
        check_same_plan(&files, expected)?;
        let targets: Vec<RepoPath> = files.iter().map(|f| f.path.clone()).collect();
        self.check_discard_index(&files)?;
        self.check_real_dirs(&targets)?;
        self.check_index_unlocked()?;

        let stash = self.stash_create()?;
        let raw = self.save_raw(&stash, &targets)?;
        let backup = self.create_backup_ref(
            "discard",
            &raw.unwrap_or(stash).to_string(),
            "project: backup of the worktree before worktree.discard",
        )?;
        self.check_discard_index(&files)?;
        // Last step before writing: the backup has exactly what is there now.
        self.check_real_dirs(&targets)?;
        let missing = self.not_as_in_backup(&stash, raw.as_ref(), &targets)?;
        if !missing.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-discard-backup-incomplete",
                backup = backup,
                files = list(&missing)
            )));
        }
        let args = ["restore", "--worktree", "--no-overlay"];
        let out = self.output_with_paths(&args, &targets)?;
        if !out.status.success() {
            return Err(with_backup(failure(&args, &out), &backup));
        }
        Ok(DiscardOutcome {
            files,
            backup,
            stash,
            raw,
        })
    }

    /// [`GitBackend::restore_worktree`](project_engine::GitBackend::restore_worktree).
    pub(super) fn restore_discarded(
        &mut self,
        request: &WorktreeRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        let paths = request.paths();
        if paths.is_empty() {
            return Ok(RestoreOutcome::default());
        }
        let status = self.status_for_write()?;
        let index = self.index_listing()?;
        let mut conflicts = request.index_conflicts(|p| index.get(p).map(|e| (e.mode, e.oid)));
        // Before reading the files: a file in place of a folder gives a clear refusal.
        self.check_real_dirs(&paths)?;
        let changed: Vec<RepoPath> = if request.redo {
            // As the undo left it: the backup bytes.
            self.not_as_in_backup(&request.stash, request.raw.as_ref(), &paths)?
        } else {
            paths
                .iter()
                .filter(|p| {
                    let hidden = index
                        .get(*p)
                        .is_some_and(|e| e.tag.is_ascii_lowercase() || e.tag == b'S');
                    hidden
                        || status.entries.iter().any(|e| {
                            e.path == **p
                                && (e.worktree.is_change()
                                    || !matches!(
                                        e.kind,
                                        EntryKind::Ordinary(_)
                                            | EntryKind::Renamed { .. }
                                            | EntryKind::Copied { .. }
                                    ))
                        })
                })
                .cloned()
                .collect()
        };
        conflicts.extend(changed.iter().map(|p| request.worktree_conflict(p)));
        request.conflict_error(conflicts)?;
        self.check_index_unlocked()?;

        let source = format!("--source={}", request.stash);
        let args: Vec<&str> = if request.redo {
            vec!["restore", "--worktree", "--no-overlay"]
        } else {
            vec!["restore", &source, "--worktree", "--no-overlay"]
        };
        let kept = || {
            msg!(
                "engine-discard-backup-kept",
                commit = request.raw.unwrap_or(request.stash)
            )
        };
        let out = self.output_with_paths(&args, &paths)?;
        if !out.status.success() {
            return Err(EngineError::GitWithReason {
                command: "restore".to_owned(),
                reason: msg!(
                    "engine-note-semicolon",
                    text = stderr_text(&out),
                    note = kept()
                ),
            });
        }
        if !request.redo
            && let Some(raw) = &request.raw
        {
            // The files saved byte for byte: those bytes are rewritten, without filters.
            for (path, entry) in self.ls_tree(raw, &paths)? {
                let content = self.blob(&entry.1)?;
                std::fs::write(self.worktree_path(&path), content).map_err(|e| {
                    EngineError::GitWithReason {
                        command: "restore".to_owned(),
                        reason: msg!(
                            "engine-note-semicolon",
                            text = msg!("engine-discard-rewrite-failed", path = path, error = e),
                            note = kept()
                        ),
                    }
                })?;
            }
        }

        // Re-read: are the files as requested?
        let wrong = if request.redo {
            self.differing(None, &paths)?
        } else {
            self.not_as_in_backup(&request.stash, request.raw.as_ref(), &paths)?
        };
        let warnings = if wrong.is_empty() {
            Vec::new()
        } else {
            vec![msg!(
                "engine-discard-not-as-expected",
                command = request.command,
                files = list(&wrong),
                n = wrong.len(),
                note = kept()
            )]
        };
        Ok(RestoreOutcome {
            backups: Vec::new(),
            warnings,
        })
    }

    fn worktree_path(&self, path: &RepoPath) -> PathBuf {
        self.workdir.join(os_from_bytes(path.as_bytes()))
    }

    /// Every folder in the path of every file is a real folder, and the file is not a
    /// folder. In place of a folder there can be an untracked file or link (or
    /// ignored: `git status` does not say), and `git restore` would delete it to recreate the
    /// file: nothing is written.
    fn check_real_dirs(&self, paths: &[RepoPath]) -> Result<(), EngineError> {
        let mut bad = Vec::new();
        for path in paths {
            let bytes = path.as_bytes();
            let mut blocked = false;
            for (i, _) in bytes.iter().enumerate().filter(|(_, b)| **b == b'/') {
                let dir = RepoPath::from_bytes(&bytes[..i]);
                match std::fs::symlink_metadata(self.worktree_path(&dir)) {
                    Ok(m) if m.is_dir() => {}
                    Ok(_) => {
                        bad.push(dir);
                        blocked = true;
                        break;
                    }
                    // The folder does not exist: git creates it, nothing to lose further down.
                    Err(_) => {
                        blocked = true;
                        break;
                    }
                }
            }
            if !blocked
                && std::fs::symlink_metadata(self.worktree_path(path)).is_ok_and(|m| m.is_dir())
            {
                bad.push(path.clone());
            }
        }
        if bad.is_empty() {
            return Ok(());
        }
        bad.sort();
        bad.dedup();
        Err(EngineError::NotAllowed(msg!(
            "engine-discard-in-the-way-git",
            files = list(&bad),
            n = bad.len()
        )))
    }

    /// The files of `paths` whose worktree is not the backup's: the content that `git
    /// restore --source=<stash>` would recreate (with the `smudge` filters and line-ending
    /// conversions), or the bytes saved in `raw`, and the same mode (`git diff <stash>`). A file that is not
    /// in the backup must not exist.
    fn not_as_in_backup(
        &self,
        stash: &Oid,
        raw: Option<&Oid>,
        paths: &[RepoPath],
    ) -> Result<Vec<RepoPath>, EngineError> {
        let mut out = self.differing(Some(stash), paths)?;
        let in_stash = self.ls_tree(stash, paths)?;
        let in_raw = match raw {
            Some(raw) => self.ls_tree(raw, paths)?,
            None => BTreeMap::new(),
        };
        for path in paths {
            let expected = match in_raw.get(path) {
                Some((_, oid)) => Some((false, self.blob(oid)?)),
                None => match in_stash.get(path) {
                    Some(entry) => Some(self.checkout_bytes(path, entry)?),
                    None => None,
                },
            };
            if Self::disk(&self.worktree_path(path))? != expected && !out.contains(path) {
                out.push(path.clone());
            }
        }
        out.sort();
        Ok(out)
    }

    /// What is in the worktree at `path`: `None` if nothing, `(true, target)` for a link,
    /// `(false, bytes)` for a file. A folder counts as content that never matches.
    fn disk(path: &Path) -> Result<Option<(bool, Vec<u8>)>, EngineError> {
        let io = |e: std::io::Error| EngineError::GitWithReason {
            command: "restore".to_owned(),
            reason: msg!(
                "engine-discard-unreadable",
                path = path.display(),
                error = e
            ),
        };
        let meta = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(path).map_err(io)?;
            Ok(Some((true, os_bytes(target.as_os_str()))))
        } else if meta.is_dir() {
            Ok(Some((true, b"/".to_vec())))
        } else {
            Ok(Some((false, std::fs::read(path).map_err(io)?)))
        }
    }

    /// The bytes `git restore` would write for the entry `entry` (mode, blob) at `path`: a link
    /// has the blob content as its target, a file goes through the filters
    /// (`cat-file --filters --path=<path>`, the attributes of that path).
    fn checkout_bytes(
        &self,
        path: &RepoPath,
        entry: &(u32, Oid),
    ) -> Result<(bool, Vec<u8>), EngineError> {
        let (mode, oid) = entry;
        if mode & 0o170000 == 0o120000 {
            return Ok((true, self.blob(oid)?));
        }
        let mut arg = OsString::from("--path=");
        arg.push(os_from_bytes(path.as_bytes()));
        let hex = oid.to_string();
        let out = self.git.output_os(
            Some(&self.workdir),
            &[
                OsStr::new("cat-file"),
                OsStr::new("--filters"),
                &arg,
                OsStr::new(&hex),
            ],
        )?;
        if !out.status.success() {
            return Err(failure(&["cat-file"], &out));
        }
        Ok((false, out.stdout))
    }

    /// The entries (mode, id) of `paths` in the tree of commit `commit` (`ls-tree -r -z
    /// --full-tree`, paths literal); a path that does not exist is missing.
    fn ls_tree(
        &self,
        commit: &Oid,
        paths: &[RepoPath],
    ) -> Result<BTreeMap<RepoPath, (u32, Oid)>, EngineError> {
        let commit = commit.to_string();
        let args = [
            "--literal-pathspecs",
            "ls-tree",
            "-r",
            "-z",
            "--full-tree",
            "--end-of-options",
            &commit,
            "--",
        ];
        let os: Vec<OsString> = paths.iter().map(|p| os_from_bytes(p.as_bytes())).collect();
        let os: Vec<&OsStr> = os.iter().map(OsString::as_os_str).collect();
        let out = self.git.run_with_paths(Some(&self.workdir), &args, &os)?;
        let mut map = BTreeMap::new();
        for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            // `<mode> <type> <id>\t<path>`
            let bad = || super::failure_text(&args[1..], record);
            let tab = record.iter().position(|&b| b == b'\t').ok_or_else(bad)?;
            let fields: Vec<&[u8]> = record[..tab].split(|&b| b == b' ').collect();
            let [mode, _, oid] = fields[..] else {
                return Err(bad());
            };
            let mode = std::str::from_utf8(mode)
                .ok()
                .and_then(|m| u32::from_str_radix(m, 8).ok())
                .ok_or_else(bad)?;
            map.insert(
                RepoPath::from_bytes(&record[tab + 1..]),
                (mode, Oid::from_hex(oid)?),
            );
        }
        Ok(map)
    }

    /// If for some regular file the worktree bytes are not the ones `git restore`
    /// would recreate from `stash` (e.g. CRLF with `core.autocrlf=input`, which the `clean` filter removes
    /// and `smudge` does not put back), saves them without filters (`hash-object -w --no-filters`) in a
    /// commit with only those files and parent `stash`, which undo uses to rewrite them identically.
    /// `None` if not needed. A link or a file type different from the backup is an error: the
    /// backup does not represent it.
    fn save_raw(&self, stash: &Oid, paths: &[RepoPath]) -> Result<Option<Oid>, EngineError> {
        let in_stash = self.ls_tree(stash, paths)?;
        let mut raw = Vec::new();
        let mut wrong = Vec::new();
        for path in paths {
            let disk = Self::disk(&self.worktree_path(path))?;
            let entry = in_stash.get(path);
            let expected = entry.map(|e| self.checkout_bytes(path, e)).transpose()?;
            match (disk, expected, entry) {
                (a, b, _) if a == b => {}
                (Some((false, bytes)), Some((false, _)), Some((mode, _))) => {
                    raw.push((path.clone(), *mode, self.hash_blob(&bytes)?));
                }
                _ => wrong.push(path.clone()),
            }
        }
        if !wrong.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-discard-backup-incomplete-stash",
                stash = stash,
                files = list(&wrong)
            )));
        }
        if raw.is_empty() {
            return Ok(None);
        }
        let index = super::TempIndex::new(&self.git_dir);
        let index_env = [("GIT_INDEX_FILE", index.0.as_os_str())];
        let mut info = Vec::new();
        for (path, mode, oid) in &raw {
            info.extend_from_slice(format!("{mode:o} {oid}\t").as_bytes());
            info.extend_from_slice(path.as_bytes());
            info.push(0);
        }
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
        let tree = String::from_utf8_lossy(tree.trim_ascii_end()).into_owned();
        let parent = stash.to_string();
        let mut message = String::from(
            "Byte-for-byte backup of the worktree, without filters, before worktree.discard\n\n\
             The parent is the `git stash create` commit; here are the files whose bytes are not \
             the ones git would recreate from it:\n\n",
        );
        for (path, _, _) in &raw {
            message.push_str(&format!("{path}\n"));
        }
        let identity: Vec<(&str, &OsStr)> = super::BACKUP_IDENTITY
            .iter()
            .map(|(k, v)| (*k, OsStr::new(v)))
            .collect();
        let args = [
            "commit-tree",
            "--no-gpg-sign",
            "-p",
            &parent,
            "-F",
            "-",
            &tree,
        ];
        let commit = self.run_input(&args, message.as_bytes(), &identity)?;
        Ok(Some(Oid::from_hex(commit.trim_ascii_end())?))
    }

    /// The index entries of the files to discard are still those of the status (mode and id):
    /// if another git process changed them, error without writing.
    fn check_discard_index(&self, files: &[DiscardedFile]) -> Result<(), EngineError> {
        let index = self.index_listing()?;
        let same = files
            .iter()
            .all(|f| index.get(&f.path).map(|e| (e.mode, e.oid)) == Some((f.mode, f.oid)));
        if same {
            Ok(())
        } else {
            Err(EngineError::NotAllowed(msg!("engine-discard-index-moved")))
        }
    }

    /// `git stash create`: the commit with the current worktree and index (touches neither
    /// of them, nor `refs/stash`). Without `core.fsmonitor`, which if stale would
    /// skip a changed file, and with the fixed backup identity (works even without
    /// `user.name`).
    fn stash_create(&self) -> Result<Oid, EngineError> {
        let args = [
            "-c",
            "core.fsmonitor=false",
            "stash",
            "create",
            STASH_MESSAGE,
        ];
        let identity: Vec<(&str, &OsStr)> = super::BACKUP_IDENTITY
            .iter()
            .map(|(k, v)| (*k, OsStr::new(v)))
            .collect();
        let out = self
            .git
            .output_with_input_env(Some(&self.workdir), &args, b"", &identity)?;
        if !out.status.success() {
            // Index locked in the meantime: `IndexLocked`, as for the other writes.
            return Err(match failure(&args, &out) {
                EngineError::Git { command, stderr } => EngineError::GitWithReason {
                    command,
                    reason: msg!(
                        "engine-note-semicolon",
                        text = stderr,
                        note = msg!("engine-discard-no-backup")
                    ),
                },
                other => other,
            });
        }
        let hex = out.stdout.trim_ascii();
        if hex.is_empty() {
            // No changes according to git: the status read earlier is no longer the current one.
            return Err(EngineError::NotAllowed(msg!(
                "engine-discard-nothing-saved"
            )));
        }
        Ok(Oid::from_hex(hex)?)
    }

    /// The files of `paths` whose worktree differs from commit `commit` (or from the index, with
    /// `None`): `git diff --name-only -z [<commit>] -- <paths>`, without fsmonitor, renames,
    /// external diffs or textconv. Here `diff.autoRefreshIndex=false` is **not** used (unlike the
    /// other reads): without the content comparison that option turns off, a file
    /// changed only in its `stat` data (rewritten identical) would appear among the differing ones.
    /// `--no-optional-locks` still prevents git from rewriting the index to update them.
    fn differing(
        &self,
        commit: Option<&Oid>,
        paths: &[RepoPath],
    ) -> Result<Vec<RepoPath>, EngineError> {
        let commit = commit.map(Oid::to_string);
        let mut args = vec![
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "--literal-pathspecs",
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=none",
        ];
        if let Some(commit) = &commit {
            args.extend(["--end-of-options", commit]);
        }
        args.push("--");
        let os: Vec<OsString> = paths.iter().map(|p| os_from_bytes(p.as_bytes())).collect();
        let os: Vec<&OsStr> = os.iter().map(OsString::as_os_str).collect();
        let out = self.git.run_with_paths(Some(&self.workdir), &args, &os)?;
        Ok(out
            .split(|&b| b == 0)
            .filter(|r| !r.is_empty())
            .map(RepoPath::from_bytes)
            .collect())
    }
}

/// Appends to a git error the backup ref, which stays.
fn with_backup(err: EngineError, backup: &str) -> EngineError {
    match err {
        EngineError::Git { command, stderr } => EngineError::GitWithReason {
            command,
            reason: msg!(
                "engine-note-semicolon",
                text = stderr,
                note = msg!("engine-discard-backup-saved", backup = backup)
            ),
        },
        other => other,
    }
}

fn list(paths: &[RepoPath]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
