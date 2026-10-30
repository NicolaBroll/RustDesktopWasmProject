//! `branch.reset` and its undo/redo on the [`CliBackend`] (steps and guarantees in
//! `impl-docs/git/reset.md`).
//!
//! In this order, and nothing is written if a step before the ref move fails:
//!
//! 1. [`RefRestore::check`](project_engine::RefRestore::check) (operation in progress), HEAD and
//! the branch equal to the expected
//!    values, the target a commit (`rev-parse --verify <oid>^{commit}`);
//! 2. status without `core.fsmonitor`; `strict` (undo/redo): nothing that would need a backup;
//! 3. `mixed`/`hard`: no `--assume-unchanged`/`--skip-worktree` file whose index entry differs
//!    from the target; `hard`: the plan ([`reset_files`]) equal to the confirmed one, no
//! untracked/ignored file (or link, or folder)
//!    where the target puts a file or a folder; `index.lock` → [`EngineError::IndexLocked`];
//! 4. backups: `mixed` the staged versions that exist only in the index (the index backup of
//!    stage/unstage); `hard` the local changes with `git stash create` (with conflicts, on a
//!    temporary copy of the index where each conflict is collapsed to "ours", so git can write
//!    it) plus the byte-for-byte copy of `worktree.discard`, the ref
//!    `refs/project/backup/reset-...`, and the last verification that the backup has exactly
//!    the current worktree;
//! 5. the ref move with the transaction of `edit.undo` (`update-ref --stdin` with the old
//!    value, backups of the commits no longer reachable);
//! 6. HEAD re-read: it must be the requested one, otherwise index and worktree are not touched;
//! 7. `mixed`: `git read-tree --reset <target>` (and a refresh); `hard`: `git read-tree --reset
//!    -u --no-recurse-submodules <target>`, which never touches untracked files outside the
//!    paths checked at step 3. No hooks run.
//!
//! An error after step 5 names the backups: the branch has moved.

use super::CliBackend;
use crate::command::{failure, os_from_bytes, path_from_output};
use project_domain::{EntryKind, Oid, RepoPath, Status};
use project_engine::{
    EngineError, Message, ResetMode, ResetOutcome, ResetRequest, check_same_reset_files,
    reset_files, reset_moved, status_paths, strict_conflicts,
};

/// Commit message of `git stash create` for a reset: fixed, no user data.
const STASH_MESSAGE: &str = "project: backup of the worktree before branch.reset";

impl CliBackend {
    /// [`GitRefs::reset`](project_engine::GitRefs::reset).
    pub(super) fn reset_head(
        &mut self,
        request: &ResetRequest,
    ) -> Result<ResetOutcome, EngineError> {
        let (_, before, target) = request.shape()?;
        request.refs.check(self.operation())?;
        let head = self.head_state()?;
        let conflicts = request.refs.conflicts(&head, |name| self.ref_value(name))?;
        if !conflicts.is_empty() {
            return Err(reset_moved(request, conflicts));
        }
        if self.resolve(&target.to_string())? != Some(target) {
            return Err(EngineError::NotAllowed(msg!(
                "engine-reset-not-commit",
                target = target
            )));
        }
        let status = self.status_for_write()?;
        if request.strict {
            let conflicts = strict_conflicts(&status, request.mode)?;
            if !conflicts.is_empty() {
                return Err(EngineError::UndoConflict {
                    command: request.refs.command,
                    refs: conflicts,
                });
            }
        }
        let mut outcome = ResetOutcome::default();
        match request.mode {
            ResetMode::Soft => {}
            ResetMode::Mixed => {
                self.check_hidden_files(&target)?;
                self.check_index_unlocked()?;
                if !request.strict {
                    let index = self.index_listing()?;
                    outcome.index_backup =
                        self.backup("branch.reset", &status, &index, &status_paths(&status))?;
                }
            }
            ResetMode::Hard => {
                let files = reset_files(&status)?;
                if !request.strict {
                    check_same_reset_files(&files, &request.files)?;
                }
                self.check_hidden_files(&target)?;
                self.check_reset_in_the_way(&target, &files)?;
                self.check_index_unlocked()?;
                if !files.is_empty() {
                    let (backup, stash, raw) = self.save_reset_backup(&status, &files)?;
                    outcome.backup = Some(backup);
                    outcome.stash = Some(stash);
                    outcome.raw = raw;
                }
                outcome.files = files;
            }
        }

        let saved = || -> Vec<String> {
            outcome
                .backup
                .iter()
                .chain(&outcome.index_backup)
                .cloned()
                .collect()
        };
        let moved = self
            .restore(&request.refs)
            .map_err(|e| match e {
                EngineError::UndoConflict { refs, .. } => reset_moved(request, refs),
                other => other,
            })
            .map_err(|e| with_backups(e, &saved(), None))?;
        outcome.backups = moved.backups;
        outcome.warnings = moved.warnings;
        if request.mode == ResetMode::Soft {
            return Ok(outcome);
        }
        // Another process changed HEAD at that very instant: the index and the worktree belong to
        // something else now, they are not touched.
        let now = self.head_state()?;
        if now != request.refs.head_target {
            return Err(with_backups(
                EngineError::NotAllowed(msg!(
                    "engine-reset-head-changed-after",
                    command = request.refs.command,
                    now = now,
                    target = request.refs.head_target
                )),
                &saved(),
                Some(&before),
            ));
        }
        let hex = target.to_string();
        let args: Vec<&str> = match request.mode {
            ResetMode::Hard => vec![
                "-c",
                "core.fsmonitor=false",
                "read-tree",
                "--reset",
                "-u",
                "--no-recurse-submodules",
                &hex,
            ],
            _ => vec!["-c", "core.fsmonitor=false", "read-tree", "--reset", &hex],
        };
        let out = self.git.output(Some(&self.workdir), &args)?;
        if !out.status.success() {
            return Err(with_backups(failure(&args, &out), &saved(), Some(&before)));
        }
        if request.mode == ResetMode::Mixed {
            // Only the `stat` data: a failure (files that differ) is the normal case.
            let _ = self.git.output(
                Some(&self.workdir),
                &[
                    "-c",
                    "core.fsmonitor=false",
                    "update-index",
                    "-q",
                    "--refresh",
                ],
            );
        }
        Ok(outcome)
    }

    /// Before a `mixed` or `hard`: git does not look at the worktree of `--assume-unchanged` files,
    /// and `read-tree --reset` refuses (after the ref has moved) to rewrite the index entry of a
    /// hidden file with local changes. Refused if one of them differs between the index and the
    /// target (`diff-index --cached`); `--skip-worktree` counts only without sparse checkout, where
    /// those files are not in the worktree and git handles them.
    fn check_hidden_files(&self, target: &Oid) -> Result<(), EngineError> {
        let sparse = self
            .git
            .output(
                Some(&self.workdir),
                &["config", "--bool", "core.sparseCheckout"],
            )?
            .stdout
            .trim_ascii()
            == b"true";
        let listing = self.index_listing()?;
        let hidden: Vec<&RepoPath> = listing
            .iter()
            .filter(|(_, e)| e.tag.is_ascii_lowercase() || (!sparse && e.tag == b'S'))
            .map(|(p, _)| p)
            .collect();
        if hidden.is_empty() {
            return Ok(());
        }
        let to = target.to_string();
        let args = [
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "diff-index",
            "--cached",
            "-z",
            "--name-only",
            "--no-renames",
            "--end-of-options",
            &to,
            "--",
        ];
        let out = self.run(&args)?;
        let hit: Vec<String> = out
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(RepoPath::from_bytes)
            .filter(|p| hidden.contains(&p))
            .map(|p| format!("`{p}`"))
            .collect();
        if hit.is_empty() {
            Ok(())
        } else {
            Err(EngineError::NotAllowed(msg!(
                "engine-reset-hidden",
                files = hit.join(", ")
            )))
        }
    }

    /// Before a `hard`: nothing untracked or ignored where the target puts something. For every
    /// path of the target that is not in the index (`diff-index --cached --diff-filter=D`) and is
    /// not one of the `files` saved in the backup: nothing must exist at that path, and every
    /// folder in its path must be a real folder or a tracked file (which the reset replaces).
    /// `git read-tree --reset -u` would overwrite or delete what it finds there.
    fn check_reset_in_the_way(&self, target: &Oid, files: &[RepoPath]) -> Result<(), EngineError> {
        let hex = target.to_string();
        let args = [
            "--no-optional-locks",
            "diff-index",
            "--cached",
            "-z",
            "--name-only",
            "--no-renames",
            "--diff-filter=D",
            "--end-of-options",
            &hex,
            "--",
        ];
        let out = self.run(&args)?;
        let index = self.index_listing()?;
        let tracked = |p: &RepoPath| index.contains_key(p) || files.contains(p);
        let mut bad = Vec::new();
        for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            let path = RepoPath::from_bytes(record);
            if files.contains(&path) {
                continue;
            }
            for (i, _) in record.iter().enumerate().filter(|(_, b)| **b == b'/') {
                let dir = RepoPath::from_bytes(&record[..i]);
                match std::fs::symlink_metadata(self.workdir.join(os_from_bytes(dir.as_bytes()))) {
                    Ok(m) if m.is_dir() => {}
                    Ok(_) if tracked(&dir) => break,
                    Ok(_) => {
                        bad.push(dir);
                        break;
                    }
                    Err(_) => break,
                }
            }
            match std::fs::symlink_metadata(self.workdir.join(os_from_bytes(record))) {
                // A folder of tracked files that the target replaces with a file: git removes
                // them (they are in HEAD or in the backup); only untracked or ignored content
                // inside it would be lost.
                Ok(m) if m.is_dir() => {
                    if self.has_untracked_inside(&path)? {
                        bad.push(path);
                    }
                }
                Ok(_) => bad.push(path),
                Err(_) => {}
            }
        }
        // A tracked file of the plan replaced by a folder: what is inside is not in the backup.
        for path in files {
            if std::fs::symlink_metadata(self.workdir.join(os_from_bytes(path.as_bytes())))
                .is_ok_and(|m| m.is_dir())
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
            "engine-reset-in-the-way",
            files = bad
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    /// True if the folder `dir` contains untracked or ignored files (`ls-files -o`, without
    /// excludes: ignored ones count too; empty folders do not).
    fn has_untracked_inside(&self, dir: &RepoPath) -> Result<bool, EngineError> {
        let os = os_from_bytes(dir.as_bytes());
        let args = [
            "--literal-pathspecs",
            "ls-files",
            "-o",
            "-z",
            "--directory",
            "--no-empty-directory",
            "--",
        ];
        let out = self
            .git
            .run_with_paths(Some(&self.workdir), &args, &[os.as_os_str()])?;
        Ok(out.iter().any(|&b| b != 0))
    }

    /// The backup of the local changes before a `hard`: `git stash create` (on a temporary copy
    /// of the index if there are conflicts), the byte-for-byte copy of the files git would not
    /// recreate identically, the ref `refs/project/backup/reset-...` and the last verification.
    /// Returns the ref, the stash commit and the raw commit.
    fn save_reset_backup(
        &self,
        status: &Status,
        files: &[RepoPath],
    ) -> Result<(String, Oid, Option<Oid>), EngineError> {
        let conflicted = status
            .entries
            .iter()
            .any(|e| matches!(e.kind, EntryKind::Unmerged(_)));
        let stash = if conflicted {
            let index = super::TempIndex::new(&self.git_dir);
            self.collapsed_index(&index.0)?;
            self.stash_create_with(STASH_MESSAGE, &[("GIT_INDEX_FILE", index.0.as_os_str())])?
        } else {
            self.stash_create_with(STASH_MESSAGE, &[])?
        };
        let raw = self.save_raw(&stash, files, "branch.reset")?;
        let backup = self.create_backup_ref(
            "reset",
            &raw.unwrap_or(stash).to_string(),
            "project: backup of the worktree before branch.reset",
        )?;
        let missing = self.not_as_in_backup(&stash, raw.as_ref(), files)?;
        if !missing.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-reset-backup-incomplete",
                backup = backup,
                files = missing
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok((backup, stash, raw))
    }

    /// Writes at `path` a copy of the index where every conflicted file has a single entry: the
    /// "ours" version (stage 2), otherwise "theirs" (stage 3), otherwise none. `git stash create`
    /// cannot save an index with conflicts; with this one it saves the worktree of the
    /// conflicted files (with their markers) like any other change.
    fn collapsed_index(&self, path: &std::path::Path) -> Result<(), EngineError> {
        let real = self.run(&["rev-parse", "--git-path", "index"])?;
        let real = self.workdir.join(path_from_output(&real));
        std::fs::copy(&real, path).map_err(|e| EngineError::GitWithReason {
            command: "stash".to_owned(),
            reason: msg!("engine-reset-index-copy", path = real.display(), error = e),
        })?;
        let args = ["ls-files", "-s", "-u", "-z", "--"];
        let out = self.run(&args)?;
        // Path → (stage, mode, id) of the best stage found.
        let mut best: std::collections::BTreeMap<Vec<u8>, (u8, String, String)> =
            std::collections::BTreeMap::new();
        for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            // `<mode> <id> <stage>\t<path>`
            let bad = || super::failure_text(&args, record);
            let tab = record.iter().position(|&b| b == b'\t').ok_or_else(bad)?;
            let fields: Vec<&[u8]> = record[..tab].split(|&b| b == b' ').collect();
            let [mode, oid, [stage]] = fields[..] else {
                return Err(bad());
            };
            let rank = match stage {
                b'2' => 3,
                b'3' => 2,
                _ => 1,
            };
            let entry =
                best.entry(record[tab + 1..].to_vec())
                    .or_insert((0, String::new(), String::new()));
            if rank > entry.0 {
                *entry = (
                    rank,
                    String::from_utf8_lossy(mode).into_owned(),
                    String::from_utf8_lossy(oid).into_owned(),
                );
            }
        }
        let zero = "0".repeat(self.empty_tree.len());
        let mut info = Vec::new();
        for (p, (rank, mode, oid)) in &best {
            info.extend_from_slice(format!("0 {zero}\t").as_bytes());
            info.extend_from_slice(p);
            info.push(0);
            if *rank > 1 {
                info.extend_from_slice(format!("{mode} {oid}\t").as_bytes());
                info.extend_from_slice(p);
                info.push(0);
            }
        }
        let env = [("GIT_INDEX_FILE", path.as_os_str())];
        let args = [
            "-c",
            "core.splitIndex=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "index.sparse=false",
            "update-index",
            "-z",
            "--index-info",
        ];
        self.run_input(&args, &info, &env)?;
        Ok(())
    }
}

/// Appends to an error the backups that stay (`moved`: the branch has already moved, from that
/// commit).
fn with_backups(err: EngineError, backups: &[String], moved: Option<&Oid>) -> EngineError {
    if backups.is_empty() && moved.is_none() {
        return err;
    }
    let names = if backups.is_empty() {
        "-".to_owned()
    } else {
        backups
            .iter()
            .map(|b| format!("`{b}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let note: Message = match moved {
        Some(before) => msg!("engine-reset-note-moved", before = before, backups = names),
        None => msg!("engine-reset-note-backups", backups = names),
    };
    match err {
        EngineError::Git { command, stderr } => EngineError::GitWithReason {
            command,
            reason: msg!("engine-note-semicolon", text = stderr, note = note),
        },
        EngineError::GitWithReason { command, reason } => EngineError::GitWithReason {
            command,
            reason: msg!("engine-note-semicolon", text = reason, note = note),
        },
        EngineError::NotAllowed(reason) => {
            EngineError::NotAllowed(msg!("engine-note-semicolon", text = reason, note = note))
        }
        other => other,
    }
}
