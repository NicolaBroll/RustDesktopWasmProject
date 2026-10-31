//! Squash merge on the [`CliBackend`] (`merge.start` with `squash`, S58), following §5 and
//! `impl-docs/git/merge.md`.
//!
//! After the checks and the backup of `merge.start` (same as a merge commit), the
//! [`SquashRecord`] in `<git dir>/project-squash.json`, then `git -c core.fsmonitor=false merge
//! --squash --no-commit --ff --no-autostash --no-overwrite-ignore --no-edit --end-of-options
//! <oid>` (`--no-commit` and `--ff` against `branch.<b>.mergeOptions` with `--commit`/`--no-ff`
//! and `merge.ff=only`, which would make git refuse; `--no-autostash` against
//! `merge.autoStash`), then the ordinary commit of what git staged through
//! [`CliBackend::create_commit`] (hooks and signing as `commit.create`).
//!
//! Git has no "squash in progress": the record makes it one ([`Operation::Squash`]) while HEAD is
//! still on the branch at the value before and `SQUASH_MSG` exists; a stale record (a commit or
//! a reset from the terminal) is ignored and removed when read.

use super::CliBackend;
use super::sync::overwrite_error;
use project_domain::Operation;
use project_engine::{
    CommitRequest, EngineError, MergeOutcome, MergeRequest, SQUASH_RECORD, SquashRecord, branch_ref,
};
use std::ffi::OsStr;

impl CliBackend {
    /// The squash of `request` (mode [`MergeMode::Squash`](project_engine::MergeMode::Squash)),
    /// after the checks of [`CliBackend::merge_into_head`] and the backup `backup` of the branch
    /// `full`.
    pub(super) fn squash_into_head(
        &mut self,
        request: &MergeRequest,
        full: &str,
        backup: String,
    ) -> Result<MergeOutcome, EngineError> {
        let record = SquashRecord::of(request, &backup);
        if let Err(e) = std::fs::write(self.git_dir.join(SQUASH_RECORD), record.to_bytes()) {
            self.drop_merge_backup(&backup, full, &request.expected);
            return Err(EngineError::GitWithReason {
                command: "merge".to_owned(),
                reason: msg!("engine-merge-squash-record-not-written", error = e),
            });
        }
        let target = request.target.to_string();
        let args: Vec<&OsStr> = [
            "-c",
            "core.fsmonitor=false",
            "merge",
            "--squash",
            "--no-commit",
            "--ff",
            "--no-autostash",
            "--no-overwrite-ignore",
            "--no-edit",
            "--end-of-options",
            &target,
        ]
        .map(OsStr::new)
        .to_vec();
        let env: [(&str, &OsStr); 2] = [
            ("GIT_REFLOG_ACTION", OsStr::new("project: merge.start")),
            ("GIT_EDITOR", OsStr::new(":")),
        ];
        let out = self.git.output_os_env(Some(&self.workdir), &args, &env)?;
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .trim()
        .to_owned();
        // `--squash` never moves HEAD: if it moved, something else happened.
        self.check_squash_head(request, full, &backup, &text)?;
        if self.operation() != Some(Operation::Squash) {
            // Git did not start the squash (no `SQUASH_MSG`): e.g. a file that would be
            // overwritten. Nothing written.
            self.remove_squash_record();
            self.drop_merge_backup(&backup, full, &request.expected);
            if let Some(error) = overwrite_error(&text, msg!("engine-merge-what-merge")) {
                return Err(error);
            }
            return Err(EngineError::Git {
                command: "merge".to_owned(),
                stderr: text,
            });
        }
        let stopped = |conflicts, message| MergeOutcome {
            after: request.expected,
            backup: backup.clone(),
            in_progress: true,
            conflicts,
            message,
            warnings: Vec::new(),
        };
        if !out.status.success() {
            // Conflicts: the squash stays in progress, with its record.
            return Ok(stopped(self.stopped_conflicts(&text), text));
        }
        if !self
            .status_for_write()?
            .entries
            .iter()
            .any(|e| e.is_staged())
        {
            // The changes are already on the branch: nothing to commit. Git wrote only
            // `SQUASH_MSG`.
            let _ = std::fs::remove_file(self.git_dir.join("SQUASH_MSG"));
            self.remove_squash_record();
            self.drop_merge_backup(&backup, full, &request.expected);
            return Ok(MergeOutcome {
                after: request.expected,
                backup,
                in_progress: false,
                conflicts: Vec::new(),
                message: String::new(),
                warnings: vec![msg!(
                    "engine-merge-squash-empty",
                    source = request.target_name,
                    branch = request.branch
                )],
            });
        }
        match self.create_commit(&CommitRequest::new(request.message.clone())) {
            Ok(_) => {
                self.remove_squash_record();
                let after = self.ref_value(full)?.unwrap_or(request.expected);
                let mut warnings = Vec::new();
                if self.head_ref_name()?.as_deref() != Some(full) {
                    warnings.push(msg!("engine-merge-head-moved-after", full = full));
                }
                Ok(MergeOutcome {
                    after,
                    backup,
                    in_progress: false,
                    conflicts: Vec::new(),
                    message: String::new(),
                    warnings,
                })
            }
            // The commit is done, something after it does not add up: the error says so, with
            // the backup of the branch.
            Err(EngineError::PostCommit { commit, reason, .. }) => {
                self.remove_squash_record();
                Err(EngineError::PostCommit {
                    commit,
                    backup: Some(backup),
                    reason,
                })
            }
            // A hook or the signing rejected the commit: the squash is staged and stays in
            // progress, to be concluded with `merge.continue` or aborted.
            Err(e) if self.operation() == Some(Operation::Squash) => {
                let error = e.to_string();
                Ok(stopped(
                    Vec::new(),
                    msg!("engine-merge-squash-commit-failed", error = error).to_string(),
                ))
            }
            Err(e) => {
                let text = e.to_string();
                self.check_squash_head(request, full, &backup, &text)?;
                Err(e)
            }
        }
    }

    /// HEAD must still be on `full` at `request.expected`; otherwise a `Git` error with git's
    /// `text` naming the backup.
    fn check_squash_head(
        &self,
        request: &MergeRequest,
        full: &str,
        backup: &str,
        text: &str,
    ) -> Result<(), EngineError> {
        let on_branch = self.head_ref_name()?.as_deref() == Some(full);
        if on_branch && self.ref_value(full)? == Some(request.expected) {
            return Ok(());
        }
        Err(EngineError::GitWithReason {
            command: "merge".to_owned(),
            reason: msg!(
                "engine-merge-squash-moved",
                full = full,
                expected = request.expected.short(),
                text = text,
                backup = backup
            ),
        })
    }

    /// Removes the [`SquashRecord`] (best effort: a stale record is ignored anyway).
    pub(super) fn remove_squash_record(&self) {
        let _ = std::fs::remove_file(self.git_dir.join(SQUASH_RECORD));
    }

    /// The record of the squash merge in progress: `None` without a record or if it is stale
    /// (no `SQUASH_MSG`, HEAD no longer on the branch at the value the squash started on, an
    /// unreadable record), and then the file is removed. If HEAD cannot be read the record is
    /// kept as valid: an operation that is not there refuses a command, never the opposite.
    pub(super) fn squash_record(&self) -> Option<SquashRecord> {
        let path = self.git_dir.join(SQUASH_RECORD);
        let bytes = std::fs::read(&path).ok()?;
        let valid = SquashRecord::from_bytes(&bytes).filter(|record| {
            if !self.git_dir.join("SQUASH_MSG").exists() {
                return false;
            }
            let full = branch_ref(&record.branch);
            match (self.head_ref_name(), self.ref_value(&full)) {
                (Ok(head), Ok(value)) => {
                    head.as_deref() == Some(full.as_str()) && value == Some(record.head)
                }
                _ => true,
            }
        });
        if valid.is_none() {
            let _ = std::fs::remove_file(&path);
        }
        valid
    }

    /// The text of the squashed commit for the abort backup message.
    pub(super) fn squash_target_text(&self) -> String {
        self.squash_record()
            .map(|r| r.target.to_string())
            .unwrap_or_default()
    }
}
