//! Cherry-pick and revert on the [`CliBackend`] (`commit.cherry_pick`, `commit.revert`, S31),
//! per §5 and `impl-docs/git/cherry-pick.md`.
//!
//! `git -c core.fsmonitor=false -c revert.reference=false <cherry-pick|revert> --no-edit
//! --no-rerere-autoupdate [-m <n>] --end-of-options <oid>`, with `GIT_REFLOG_ACTION=project:
//! <id>` e `GIT_EDITOR=:`. Hooks and signing follow the configuration (never `--no-verify`).
//!
//! - succeeded: the branch on a new commit;
//! - **stopped** (conflicts, a hook that rejects the commit): the operation **stays in progress**
//!   (`CHERRY_PICK_HEAD`/`REVERT_HEAD`), conflicted files from the status and the output;
//! - empty (commit already applied): `git <cmd> --skip` if git left it in progress, then
//!   [`EngineError::NotAllowed`] and the backup removed;
//! - not started (a file that would be overwritten): nothing written, the backup is removed.
//!
//! First of all the backup `refs/project/backup/<cherry-pick|revert>-...` of the branch as it was;
//! it is removed only if nothing changed.

use super::CliBackend;
use super::sync::overwrite_error;
use project_engine::{
    EngineError, MergeOutcome, PickKind, PickRequest, branch_ref, check_pick_state,
    pick_empty_error,
};
use std::ffi::OsStr;

/// True if git's text says the result is empty (nothing to commit).
fn says_empty(text: &str) -> bool {
    text.contains("is now empty")
        || text.contains("nothing to commit")
        || text.contains("--allow-empty")
}

impl CliBackend {
    /// [`GitOperations::pick`](project_engine::GitOperations::pick).
    pub(super) fn pick_onto_head(
        &mut self,
        request: &PickRequest,
    ) -> Result<MergeOutcome, EngineError> {
        let kind = request.kind;
        let id = kind.command();
        let status = self.status_for_write()?;
        check_pick_state(&status, kind)?;
        let full = branch_ref(&request.branch);
        if self.head_ref_name()?.as_deref() != Some(full.as_str()) {
            return Err(EngineError::NotAllowed(msg!(
                "engine-pick-head-moved-id",
                id = id
            )));
        }
        self.check_branch_value(&full, &request.expected)?;
        // The patch to apply: from the parent to the commit (cherry-pick) or the reverse.
        let parent = request
            .parent
            .map_or_else(|| self.empty_tree.to_owned(), |p| p.to_string());
        let commit = request.commit.to_string();
        let (from, to) = match kind {
            PickKind::CherryPick => (parent, commit.clone()),
            PickKind::Revert => (commit.clone(), parent),
        };
        self.check_assume_unchanged_between(&from, &to)?;
        self.check_pick_paths(&request.expected.to_string(), &from, &to, id)?;
        let expected = request.expected.to_string();
        let had_msg = self.git_dir.join("MERGE_MSG").exists();
        let backup = self.create_backup_ref(
            kind.git(),
            &expected,
            &format!("project: backup of {full} before {id}"),
        )?;
        let reflog = format!("project: {id}");
        let env: [(&str, &OsStr); 2] = [
            ("GIT_REFLOG_ACTION", OsStr::new(&reflog)),
            ("GIT_EDITOR", OsStr::new(":")),
        ];
        let mainline = request.mainline.map(|m| m.to_string());
        let mut args: Vec<&str> = vec![
            "-c",
            "core.fsmonitor=false",
            // `revert.reference=true` would change the revert message.
            "-c",
            "revert.reference=false",
            kind.git(),
            "--no-edit",
            "--no-rerere-autoupdate",
        ];
        if let Some(m) = &mainline {
            args.extend(["-m", m.as_str()]);
        }
        args.extend(["--end-of-options", commit.as_str()]);
        let args: Vec<&OsStr> = args.into_iter().map(OsStr::new).collect();
        let out = self.git.output_os_env(Some(&self.workdir), &args, &env)?;
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .trim()
        .to_owned();
        let operation = self.operation();
        if out.status.success() && operation.is_none() {
            let after = self
                .ref_value(&full)?
                .ok_or_else(|| EngineError::GitWithReason {
                    command: kind.git().to_owned(),
                    reason: msg!("engine-pick-branch-gone", id = id, full = full),
                })?;
            if after == request.expected {
                return Err(EngineError::GitWithReason {
                    command: kind.git().to_owned(),
                    reason: msg!(
                        "engine-pick-branch-not-moved",
                        git = kind.git(),
                        full = full,
                        text = text,
                        backup = backup
                    ),
                });
            }
            let mut warnings = Vec::new();
            if self.head_ref_name()?.as_deref() != Some(full.as_str()) {
                warnings.push(msg!("engine-head-moved-after", command = id, full = full));
            }
            return Ok(MergeOutcome {
                after,
                backup,
                in_progress: false,
                conflicts: Vec::new(),
                message: String::new(),
                warnings,
            });
        }
        if operation == Some(kind.operation()) {
            let conflicts = self.stopped_conflicts(&text);
            if conflicts.is_empty() && says_empty(&text) && self.index_matches_head()? {
                // Empty: git leaves the operation in progress with nothing to commit. The commit
                // is skipped (`--skip`: with a single commit it resets index and worktree to HEAD,
                // which were clean) and nothing stays in progress.
                return self.skip_empty(request, &full, &backup, had_msg);
            }
            return Ok(MergeOutcome {
                after: request.expected,
                backup,
                in_progress: true,
                conflicts,
                message: text,
                warnings: Vec::new(),
            });
        }
        // Nothing in progress: nothing started, or the result was empty (reverting an
        // already reverted commit ends this way).
        let unchanged = operation.is_none();
        if unchanged {
            self.drop_merge_backup(&backup, &full, &request.expected);
            if !had_msg {
                self.remove_stale_merge_msg();
            }
        }
        if unchanged && says_empty(&text) {
            return Err(pick_empty_error(kind, &request.commit));
        }
        if let Some(error) = overwrite_error(&text, msg!("engine-command-quoted", id = id)) {
            return Err(error);
        }
        Err(if unchanged {
            EngineError::Git {
                command: kind.git().to_owned(),
                stderr: text,
            }
        } else {
            EngineError::GitWithReason {
                command: kind.git().to_owned(),
                reason: msg!(
                    "engine-note-newline",
                    text = text,
                    note = msg!("engine-pick-backup-kept", backup = backup)
                ),
            }
        })
    }

    /// True if the index equals HEAD: the `says_empty` text can also come from a hook, and
    /// `--skip` would throw away a staged result.
    pub(super) fn index_matches_head(&self) -> Result<bool, EngineError> {
        let out = self.git.output(
            Some(&self.workdir),
            &["diff-index", "--cached", "--quiet", "HEAD", "--"],
        )?;
        Ok(out.status.success())
    }

    /// The empty result left in progress: `git <cmd> --skip`, then
    /// [`pick_empty_error`] with the backup removed; if git does not close it, an error naming the
    /// backup (which stays).
    fn skip_empty(
        &mut self,
        request: &PickRequest,
        full: &str,
        backup: &str,
        had_msg: bool,
    ) -> Result<MergeOutcome, EngineError> {
        let kind = request.kind;
        let reflog = format!("project: {}", kind.command());
        let env = [("GIT_REFLOG_ACTION", OsStr::new(&reflog))];
        let args = ["-c", "core.fsmonitor=false", kind.git(), "--skip"];
        let out = self
            .git
            .output_with_input_env(Some(&self.workdir), &args, b"", &env)?;
        if !out.status.success() || self.operation().is_some() {
            return Err(EngineError::GitWithReason {
                command: kind.git().to_owned(),
                reason: msg!(
                    "engine-pick-skip-failed",
                    command = kind.command(),
                    commit = request.commit,
                    git = kind.git(),
                    stderr = String::from_utf8_lossy(&out.stderr).trim(),
                    backup = backup
                ),
            });
        }
        self.drop_merge_backup(backup, full, &request.expected);
        if !had_msg {
            self.remove_stale_merge_msg();
        }
        Err(pick_empty_error(kind, &request.commit))
    }

    /// Removes the `MERGE_MSG` left by a cherry-pick or revert that did not start or was empty
    /// (git leaves it even with no operation in progress, and a later `git commit` would
    /// propose it as the message). Only with no operation in progress.
    fn remove_stale_merge_msg(&self) {
        if self.operation().is_none() {
            let _ = std::fs::remove_file(self.git_dir.join("MERGE_MSG"));
        }
    }

    /// Files that the patch `from` → `to` writes and that HEAD (`head`) does not have must not exist
    /// in the worktree (untracked or ignored): git would overwrite them.
    fn check_pick_paths(
        &self,
        head: &str,
        from: &str,
        to: &str,
        what: &str,
    ) -> Result<(), EngineError> {
        let diff = |a: &str, b: &str, filter: &str| -> Result<Vec<Vec<u8>>, EngineError> {
            let args = [
                "diff-tree",
                "-r",
                "-z",
                "--name-only",
                "--no-renames",
                filter,
                "--end-of-options",
                a,
                b,
            ];
            Ok(self
                .run(&args)?
                .split(|&b| b == 0)
                .filter(|p| !p.is_empty())
                .map(<[u8]>::to_vec)
                .collect())
        };
        let written = diff(from, to, "--diff-filter=AMT")?;
        if written.is_empty() {
            return Ok(());
        }
        let not_in_head = diff(head, to, "--diff-filter=A")?;
        let paths: Vec<Vec<u8>> = written
            .into_iter()
            .filter(|p| not_in_head.contains(p))
            .collect();
        self.check_paths_free(&paths, msg!("engine-command-quoted", id = what))
    }
}
