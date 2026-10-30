//! Merge on the [`CliBackend`] (`merge.start`, `merge.abort`, S29) and reading of the operation in
//! progress (`repo.operation`), following §5 and `impl-docs/git/merge.md`.
//!
//! - fast-forward: `git -c core.fsmonitor=false merge --ff-only --no-autostash
//!   --no-overwrite-ignore --no-edit --end-of-options <oid>` (git itself refuses, before
//!   writing, to overwrite local changes and untracked files);
//! - merge commit: `git -c core.fsmonitor=false merge --no-autostash --no-overwrite-ignore
//!   --no-edit --commit --no-squash --ff|--no-ff -F <file> --end-of-options <oid>`, the message
//!   from a temporary file in the git folder (git does not read `-F -` from stdin for `merge`);
//! - **stopped** (conflicts, `pre-merge-commit` hook): the merge **stays in progress**, the
//!   conflicted files from the status and from git's output; no `--abort`;
//! - not started (a file that would be overwritten): nothing written, the backup is removed.
//!
//! First of all the backup `refs/project/backup/merge-...` of the branch as it was; removed only
//! if nothing changed. `GIT_REFLOG_ACTION=project: merge.start`, `GIT_EDITOR=:`.
//!
//! `merge.abort`: index and worktree saved in a commit like a stash (parents HEAD and the
//! index commit, built in temporary indexes), the ref
//! `refs/project/backup/merge-abort-...`, then `git merge --abort`.

use super::sync::{TempFile, overwrite_error};
use super::{BACKUP_IDENTITY, CliBackend, TempIndex};
use crate::command::os_from_bytes;
use project_domain::{Oid, Operation, RepoPath, Status};
use project_engine::{
    EngineError, MergeAborted, MergeMode, MergeOutcome, MergeRequest, OperationState, PickKind,
    branch_ref, check_merge_state, check_merging, check_picking,
};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::time::{SystemTime, UNIX_EPOCH};

/// The reflog message of the writes of `merge.start`.
const REFLOG_ACTION: &str = "project: merge.start";

/// Options for a plain temporary index (like the index backups): no split index,
/// fsmonitor or sparse index, whatever the repo configuration.
const PLAIN_INDEX: [&str; 6] = [
    "-c",
    "core.splitIndex=false",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "index.sparse=false",
];

/// The operation aborted by [`CliBackend::abort_operation`]: the type, the git subcommand
/// (also in the backup name), the command id, the file with the incoming commits.
pub(super) struct AbortTarget {
    pub(super) operation: Operation,
    pub(super) git: &'static str,
    pub(super) command: &'static str,
    pub(super) head_file: &'static str,
}

impl AbortTarget {
    const MERGE: AbortTarget = AbortTarget {
        operation: Operation::Merge,
        git: "merge",
        command: "merge.abort",
        head_file: "MERGE_HEAD",
    };
}

/// The ids, one per line, of a file like `MERGE_HEAD`; invalid lines are skipped.
pub(super) fn oids(bytes: &[u8]) -> Vec<Oid> {
    bytes
        .split(|&b| b == b'\n')
        .map(<[u8]>::trim_ascii)
        .filter(|l| !l.is_empty())
        .filter_map(|l| Oid::from_hex(l).ok())
        .collect()
}

impl CliBackend {
    /// [`GitOperations::merge`](project_engine::GitOperations::merge).
    pub(super) fn merge_into_head(
        &mut self,
        request: &MergeRequest,
    ) -> Result<MergeOutcome, EngineError> {
        let status = self.status_for_write()?;
        check_merge_state(&status, &request.mode)?;
        let full = branch_ref(&request.branch);
        if self.head_ref_name()?.as_deref() != Some(full.as_str()) {
            return Err(EngineError::NotAllowed(msg!("engine-merge-head-moved")));
        }
        self.check_branch_value(&full, &request.expected)?;
        self.check_assume_unchanged(&Some(request.expected), &request.target)?;
        self.check_incoming_paths(
            &request.expected,
            &request.target,
            false,
            msg!("engine-merge-what-merge"),
        )?;
        let expected = request.expected.to_string();
        let target = request.target.to_string();
        let backup = self.create_backup_ref(
            "merge",
            &expected,
            &format!("project: backup of {full} before merge.start"),
        )?;
        let env: [(&str, &OsStr); 2] = [
            ("GIT_REFLOG_ACTION", OsStr::new(REFLOG_ACTION)),
            ("GIT_EDITOR", OsStr::new(":")),
        ];
        let mut args: Vec<&OsStr> = ["-c", "core.fsmonitor=false", "merge"]
            .into_iter()
            .map(OsStr::new)
            .collect();
        // The message file lives until the end of the function, after `git merge`.
        let file;
        match request.mode {
            MergeMode::FastForward => {
                args.extend(
                    [
                        "--ff-only",
                        "--no-squash",
                        "--commit",
                        "--no-autostash",
                        "--no-overwrite-ignore",
                        "--no-edit",
                    ]
                    .map(OsStr::new),
                );
            }
            MergeMode::Merge { no_ff } => {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos());
                file = TempFile(
                    self.git_dir
                        .join(format!("project-merge-msg-{}-{nanos}", std::process::id())),
                );
                if let Err(e) = std::fs::write(&file.0, request.message.as_bytes()) {
                    self.drop_merge_backup(&backup, &full, &request.expected);
                    return Err(EngineError::GitWithReason {
                        command: "merge".to_owned(),
                        reason: msg!("engine-merge-message-not-written", error = e),
                    });
                }
                args.extend(
                    [
                        "--no-autostash",
                        "--no-overwrite-ignore",
                        "--no-edit",
                        "--commit",
                        "--no-squash",
                        if no_ff { "--no-ff" } else { "--ff" },
                        "-F",
                    ]
                    .map(OsStr::new),
                );
                // The path as `OsStr`: the git folder may not be UTF-8.
                args.push(file.0.as_os_str());
            }
        }
        args.extend(["--end-of-options", target.as_str()].map(OsStr::new));
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
                    command: "merge".to_owned(),
                    reason: msg!("engine-merge-branch-gone", full = full),
                })?;
            if after == request.expected {
                // Git says it finished but the branch did not move (e.g. a `--squash` from the
                // configuration, or a hook): it is not a successful merge. The backup stays.
                return Err(EngineError::GitWithReason {
                    command: "merge".to_owned(),
                    reason: msg!(
                        "engine-merge-branch-not-moved",
                        full = full,
                        text = text,
                        backup = backup
                    ),
                });
            }
            let mut warnings = Vec::new();
            if self.head_ref_name()?.as_deref() != Some(full.as_str()) {
                warnings.push(msg!("engine-merge-head-moved-after", full = full));
            }
            if !self.is_ancestor(&request.target, &after)? {
                warnings.push(msg!(
                    "engine-merge-target-missing",
                    full = full,
                    after = after,
                    target = request.target_name
                ));
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
        if operation == Some(Operation::Merge) {
            // Stopped: stays in progress. The conflicted files are those still unmerged and those
            // git reported as conflicted (even if `rerere` already resolved them).
            let conflicts = self.stopped_conflicts(&text);
            return Ok(MergeOutcome {
                after: request.expected,
                backup,
                in_progress: true,
                conflicts,
                message: text,
                warnings: Vec::new(),
            });
        }
        // Nothing started (e.g. an untracked file that would be overwritten).
        self.drop_merge_backup(&backup, &full, &request.expected);
        if let Some(error) = overwrite_error(&text, msg!("engine-merge-what-merge")) {
            return Err(error);
        }
        Err(EngineError::Git {
            command: "merge".to_owned(),
            stderr: text,
        })
    }

    /// Removes the backup `name` if the branch `full` is still `expected` (nothing changed) and
    /// the backup still points there.
    pub(super) fn drop_merge_backup(&self, name: &str, full: &str, expected: &Oid) {
        if self.ref_value(full).ok().flatten() == Some(*expected) {
            let oid = expected.to_string();
            let _ = self
                .git
                .output(Some(&self.workdir), &["update-ref", "-d", name, &oid]);
        }
    }

    /// [`GitOperations::merge_abort`](project_engine::GitOperations::merge_abort).
    pub(super) fn abort_merge(&mut self) -> Result<MergeAborted, EngineError> {
        let status = self.status_for_write()?;
        check_merging("merge.abort", &status)?;
        self.abort_operation(&status, AbortTarget::MERGE, &[])
    }

    /// [`GitOperations::pick_abort`](project_engine::GitOperations::pick_abort).
    pub(super) fn abort_pick(&mut self, kind: PickKind) -> Result<MergeAborted, EngineError> {
        let status = self.status_for_write()?;
        check_picking(
            kind.abort_command(),
            kind,
            &status,
            &self.read_operation_state(),
        )?;
        self.abort_operation(
            &status,
            AbortTarget {
                operation: kind.operation(),
                git: kind.git(),
                command: kind.abort_command(),
                head_file: kind.head_file(),
            },
            &[],
        )
    }

    /// The abort of the operation in progress `target` (already checked against the status
    /// `status`): the index and worktree backup like a stash (with the files `extra` too, e.g.
    /// untracked files the abort would overwrite), then `git <git> --abort`.
    pub(super) fn abort_operation(
        &mut self,
        status: &Status,
        target: AbortTarget,
        extra: &[RepoPath],
    ) -> Result<MergeAborted, EngineError> {
        let AbortTarget {
            operation,
            git,
            command,
            head_file,
        } = target;
        self.check_index_unlocked()?;
        let head = self
            .resolve("HEAD")?
            .ok_or_else(|| EngineError::NotAllowed(msg!("engine-merge-abort-unborn")))?;
        let mut files: Vec<RepoPath> = status
            .entries
            .iter()
            .filter(|e| e.is_conflict() || e.is_staged() || e.is_unstaged())
            .map(|e| e.path.clone())
            .collect();
        for path in extra {
            if !files.contains(path) {
                files.push(path.clone());
            }
        }
        // `git <git> --abort` (`reset --merge`) refuses a staged file that also has unstaged
        // changes: check first, like the `MemoryBackend`, so nothing is written.
        // `rebase --abort` is a `reset --hard`: no such limit.
        let mixed: Vec<String> = status
            .entries
            .iter()
            .filter(|_| operation != Operation::Rebase)
            .filter(|e| e.is_staged() && e.is_unstaged())
            .map(|e| format!("`{}`", e.path))
            .collect();
        if !mixed.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-merge-abort-mixed",
                files = mixed.join(", "),
                command = command
            )));
        }
        let backup = self.save_merge_backup(&head, &files, git, command, head_file)?;
        let reflog = format!("project: {command}");
        let env = [("GIT_REFLOG_ACTION", OsStr::new(&reflog))];
        let args = ["-c", "core.fsmonitor=false", git, "--abort"];
        let out = self
            .git
            .output_with_input_env(Some(&self.workdir), &args, b"", &env)?;
        let still_merging = self.operation() == Some(operation);
        if !out.status.success() || still_merging {
            // The backup is never removed: even when git refuses, `reset --merge` may have
            // already rewritten some of the files (a file that cannot be deleted, a read-only
            // folder) with `MERGE_HEAD` and HEAD unchanged.
            let stderr = String::from_utf8_lossy(&out.stderr)
                .trim()
                .trim_end_matches('.')
                .to_owned();
            let hint = if stderr.contains("not uptodate") {
                "mixed"
            } else {
                "none"
            };
            return Err(EngineError::GitWithReason {
                command: git.to_owned(),
                reason: msg!(
                    "engine-merge-abort-failed",
                    git = git,
                    stderr = stderr,
                    hint = hint,
                    state = if still_merging { "still" } else { "maybe" },
                    backup = backup
                ),
            });
        }
        let mut warnings = Vec::new();
        // The rebase goes back to its branch: HEAD moves by design.
        if operation != Operation::Rebase && self.resolve("HEAD")? != Some(head) {
            warnings.push(msg!("engine-merge-abort-head-moved", head = head));
        }
        Ok(MergeAborted {
            backup,
            files,
            warnings,
        })
    }

    /// Saves index and worktree before `merge.abort`, like a stash: a commit with the index
    /// (the stage 0 entries, and for conflicted files our version, stage 2) with
    /// parent HEAD, and a commit with the index tree plus the files `files` as they were in the
    /// worktree (conflicts with markers and resolutions included), with parents HEAD and the
    /// index commit. All in temporary indexes: the real index, with the conflicts, is not
    /// touched. Then the new ref `refs/project/backup/merge-abort-...` on the second commit.
    fn save_merge_backup(
        &self,
        head: &Oid,
        files: &[RepoPath],
        git: &str,
        command: &str,
        head_file: &str,
    ) -> Result<String, EngineError> {
        // The real index, with the stages.
        let listing = self.run(&["ls-files", "-s", "-z", "--"])?;
        let mut info = Vec::new();
        let mut in_index: BTreeSet<Vec<u8>> = BTreeSet::new();
        for record in listing.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            let Some(tab) = record.iter().position(|&b| b == b'\t') else {
                continue;
            };
            let (meta, path) = (&record[..tab], &record[tab + 1..]);
            let mut fields = meta.split(|&b| b == b' ');
            let (Some(mode), Some(oid), Some(stage)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if stage != b"0" && stage != b"2" {
                continue;
            }
            info.extend_from_slice(mode);
            info.push(b' ');
            info.extend_from_slice(oid);
            info.push(b'\t');
            info.extend_from_slice(path);
            info.push(0);
            in_index.insert(path.to_vec());
        }
        let identity: Vec<(&str, &OsStr)> = BACKUP_IDENTITY
            .iter()
            .map(|(k, v)| (*k, OsStr::new(v)))
            .collect();
        let head = head.to_string();
        let merging = std::fs::read(self.git_dir.join(head_file))
            .map(|b| {
                oids(&b)
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();

        let index = TempIndex::new(&self.git_dir);
        let env = [("GIT_INDEX_FILE", index.0.as_os_str())];
        let update = [&PLAIN_INDEX[..], &["update-index", "-z", "--index-info"]].concat();
        self.run_input(&update, &info, &env)?;
        let write = [&PLAIN_INDEX[..], &["write-tree"]].concat();
        let index_tree = self.run_input(&write, b"", &env)?;
        let index_tree = String::from_utf8_lossy(index_tree.trim_ascii_end()).into_owned();
        let message = format!("project: index before {command} ({git} of {merging})");
        let args = [
            "commit-tree",
            "--no-gpg-sign",
            "-p",
            &head,
            "-F",
            "-",
            &index_tree,
        ];
        let index_commit = self.run_input(&args, message.as_bytes(), &identity)?;
        let index_commit = String::from_utf8_lossy(index_commit.trim_ascii_end()).into_owned();

        // The worktree: on top of the index, the changed files as they were on disk (deleted ones
        // included; a file that is neither on disk nor in the index has nothing to save).
        let paths: Vec<&RepoPath> = files
            .iter()
            .filter(|p| {
                in_index.contains(p.as_bytes())
                    || std::fs::symlink_metadata(self.workdir.join(os_from_bytes(p.as_bytes())))
                        .is_ok()
            })
            .collect();
        let worktree_tree = if paths.is_empty() {
            index_tree.clone()
        } else {
            let read = [&PLAIN_INDEX[..], &["read-tree", &index_tree]].concat();
            self.run_input(&read, b"", &env)?;
            let mut input = Vec::new();
            for path in &paths {
                input.extend_from_slice(path.as_bytes());
                input.push(0);
            }
            let add = [
                &["--literal-pathspecs"][..],
                &PLAIN_INDEX[..],
                &[
                    "add",
                    "-A",
                    "-f",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ],
            ]
            .concat();
            self.run_input(&add, &input, &env)?;
            let tree = self.run_input(&write, b"", &env)?;
            String::from_utf8_lossy(tree.trim_ascii_end()).into_owned()
        };
        let message =
            format!("project: backup of index and worktree before {command} ({git} of {merging})");
        let args = [
            "commit-tree",
            "--no-gpg-sign",
            "-p",
            &head,
            "-p",
            &index_commit,
            "-F",
            "-",
            &worktree_tree,
        ];
        let commit = self.run_input(&args, message.as_bytes(), &identity)?;
        let commit = String::from_utf8_lossy(commit.trim_ascii_end()).into_owned();
        self.create_backup_ref(
            &format!("{git}-abort"),
            &commit,
            &format!("project: backup of index and worktree before {command}"),
        )
    }

    /// [`GitOperations::operation_state`](project_engine::GitOperations::operation_state): from the files
    /// of the git folder (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `MERGE_MSG`,
    /// `rebase-merge/` or `rebase-apply/` with `onto` and `head-name`).
    pub(super) fn read_operation_state(&self) -> OperationState {
        let read = |name: &str| std::fs::read(self.git_dir.join(name)).ok();
        let operation = self.operation();
        let incoming = match operation {
            Some(Operation::Merge) => read("MERGE_HEAD"),
            Some(Operation::CherryPick) => read("CHERRY_PICK_HEAD"),
            Some(Operation::Revert) => read("REVERT_HEAD"),
            _ => None,
        }
        .map(|b| oids(&b))
        .unwrap_or_default();
        let message = match operation {
            Some(Operation::Merge | Operation::CherryPick | Operation::Revert) => {
                read("MERGE_MSG").map(|b| String::from_utf8_lossy(&b).into_owned())
            }
            _ => None,
        };
        let (mut onto, mut rebase_branch) = (None, None);
        if operation == Some(Operation::Rebase) {
            let dir = if self.git_dir.join("rebase-merge").exists() {
                "rebase-merge"
            } else {
                "rebase-apply"
            };
            onto = read(&format!("{dir}/onto")).and_then(|b| oids(&b).into_iter().next());
            rebase_branch = read(&format!("{dir}/head-name"))
                .map(|b| String::from_utf8_lossy(b.trim_ascii()).into_owned())
                .filter(|n| n.starts_with("refs/"));
        }
        OperationState {
            operation,
            incoming,
            message,
            onto,
            rebase_branch,
            sequencer: self.git_dir.join("sequencer").is_dir(),
            rebase: if operation == Some(Operation::Rebase) {
                self.read_rebase_record()
            } else {
                None
            },
        }
    }
}
