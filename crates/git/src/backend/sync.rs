//! Pull on the [`CliBackend`] (`remote.pull`, S26) and its undo/redo, following §5 and §7.
//!
//! The fetch is done by the command beforehand, with [`GitRemotes::fetch`](project_engine::GitRemotes::fetch);
//! only the local integration is here, which is not cancelled halfway:
//!
//! - fast-forward of the active branch: `git -c core.fsmonitor=false merge --ff-only
//!   --no-autostash --no-edit --end-of-options <oid>`: git itself refuses (before writing) to
//!   overwrite local changes and untracked files; for another branch: `update-ref -m ...
//!   refs/heads/<b> <new> <old>` (worktree untouched), refused if the branch is open
//!   in another worktree;
//! - merge: `git -c core.fsmonitor=false merge --no-autostash --no-edit --commit --no-squash
//!   [--ff|--no-ff] -F <file> --end-of-options <oid>`: the message from a temporary file in the
//!   git folder (git does not read `-F -` from stdin for `merge`), `--ff`/`--no-ff`, `--commit` and
//!   `--no-squash` explicit so as not to depend on `merge.ff` and `branch.<name>.mergeOptions`.
//!   Hooks (`pre-merge-commit`, `commit-msg`) and signing as the configuration says;
//! - rebase: `git -c core.fsmonitor=false -c rebase.updateRefs=false rebase --no-autostash
//!   [--rebase-merges] --end-of-options <oid>` (`rebase.updateRefs` would move other branches
//!   outside the journal), with `GIT_EDITOR=:` and `GIT_SEQUENCE_EDITOR=:`. Rebase onto the upstream
//!   like `git rebase <upstream>`, without the fork-point of `git pull --rebase`: if the upstream
//!   was rewritten (force push) its old commits are replayed too, and usually the
//!   rebase stops with a conflict;
//! - stopped merge (conflicts, `pre-merge-commit`/`commit-msg` hooks, signing): **stays in progress**
//!   as with `merge.start` ([`PullOutcome::in_progress`] with the conflicted files from the status and
//!   from git's output), to be finished with `merge.continue` or aborted with `merge.abort`;
//! - stopped rebase (S32): **stays in progress** too, with the conflicted files and the
//!   [`RebaseRecord`] of the pull in `rebase-merge/project-rebase.json`, to be finished with
//!   `rebase.continue` (the pull's journal entry) or aborted with `rebase.abort`.
//!
//! First of all the backup `refs/project/backup/pull-...` of the branch as it was; if then nothing
//! changes (error) it is removed; with a stopped merge or rebase it stays.
//! `GIT_REFLOG_ACTION=project: remote.pull` gives the reflog message.
//!
//! Undo/redo of the active branch ([`CliBackend::reset_keep`]): the same checks as
//! `restore_refs` and the backups of the abandoned commits, then `git -c core.fsmonitor=false reset
//! --keep <oid>` (updates index and worktree of the files that differ between the two commits,
//! refuses if one of them has local changes, does not overwrite untracked ones); first it checks that no
//! file to be recreated already exists in the worktree (`reset --keep` would overwrite an ignored file).
//! The instant between the ref check and `reset` remains: `reset` updates the branch with the value
//! it read itself, not with the expected one.

use super::CliBackend;
use crate::command::{failure, os_from_bytes};
use project_domain::remote_short;
use project_domain::{Commit, EntryKind, LOG_ARGS, Oid, Operation, RepoPath, parse_log};
use project_engine::{
    EngineError, HeadState, Message, PullMode, PullOperation, PullOutcome, PullRequest,
    PullSettings, RebaseRecord, RefRestore, RestoreOutcome, UpstreamConfig, branch_ref,
    check_pull_state,
};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A temporary file in the git folder (the merge message), deleted when it goes out of
/// scope.
pub(super) struct TempFile(pub(super) PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The reflog message of the writes of a pull.
const REFLOG_ACTION: &str = "project: remote.pull";

impl CliBackend {
    /// [`GitRemotes::upstream`](project_engine::GitRemotes::upstream) with `for-each-ref`:
    /// `%(upstream:remotename)` and `%(upstream:remoteref)` are `branch.<name>.remote` and
    /// `.merge`, `%(upstream)` the local remote branch according to the remote's refspec.
    pub(super) fn upstream_of(&self, branch: &str) -> Result<Option<UpstreamConfig>, EngineError> {
        let full = branch_ref(branch);
        let args = [
            "for-each-ref",
            "--format=%(refname)%00%(upstream)%00%(upstream:remotename)%00%(upstream:remoteref)",
            &full,
        ];
        let out = self.run(&args)?;
        for line in out.split(|&b| b == b'\n') {
            let fields: Vec<&[u8]> = line.split(|&b| b == 0).collect();
            let [name, tracking, remote, merge] = fields[..] else {
                continue;
            };
            if name != full.as_bytes() {
                continue;
            }
            if remote.is_empty() || merge.is_empty() {
                return Ok(None);
            }
            let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
            let tracking = text(tracking);
            let remote = text(remote);
            return Ok(Some(UpstreamConfig {
                tracking: (!tracking.is_empty() && remote != ".").then_some(tracking),
                remote,
                merge: text(merge),
            }));
        }
        Ok(None)
    }

    /// The value of `key` in the configuration (all sources, as git reads it), `None` if
    /// there is none.
    fn config_value(&self, key: &str) -> Result<Option<String>, EngineError> {
        let args = ["config", "--get", "--end-of-options", key];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.code() == Some(1) && out.stderr.is_empty() {
            return Ok(None);
        }
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        let value = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
        Ok(Some(String::from_utf8_lossy(value).into_owned()))
    }

    /// [`GitRead::config_values`](project_engine::GitRead::config_values): `git config
    /// -z --get-all`, non-UTF-8 values converted with `from_utf8_lossy`.
    pub(super) fn config_all(&self, key: &str) -> Result<Vec<String>, EngineError> {
        let args = ["config", "-z", "--get-all", "--end-of-options", key];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.code() == Some(1) && out.stderr.is_empty() {
            return Ok(Vec::new());
        }
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        Ok(out
            .stdout
            .split(|&b| b == 0)
            .filter(|v| !v.is_empty())
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .collect())
    }

    /// Before integrating the upstream into the active branch: the files the pull would bring into the
    /// worktree and that HEAD does not have (`diff-tree --diff-filter=A HEAD <upstream>`) must not
    /// exist in the worktree, neither as a file nor in place of one of their folders: they would be
    /// untracked or ignored, and git overwrites (or, with an aborted merge, deletes) ignored
    /// files without saying so. The rebase has no `--no-overwrite-ignore`; for fast-forward and merge
    /// it is a second defence. Otherwise [`EngineError::NotAllowed`] with the files.
    ///
    /// With `replay` (rebase) the files touched by each of the commits to replay also count
    /// (`git log -m --no-renames --name-only <upstream>..HEAD`, merges included for
    /// `--rebase-merges`) that HEAD does not have: the rebase goes through the intermediate trees, and a file
    /// added by one commit and removed by a later one (or renamed, or removed and recreated), or
    /// modified by a local commit but removed by the upstream (modify/delete conflict, then
    /// `rebase --abort`), would be written and then deleted, taking with it the ignored file in
    /// its place.
    pub(super) fn check_incoming_paths(
        &self,
        head: &Oid,
        upstream: &Oid,
        replay: bool,
        what: Message,
    ) -> Result<(), EngineError> {
        let (from, to) = (head.to_string(), upstream.to_string());
        let args = [
            "diff-tree",
            "-r",
            "-z",
            "--name-only",
            "--no-renames",
            "--diff-filter=A",
            "--end-of-options",
            &from,
            &to,
        ];
        let mut paths: Vec<Vec<u8>> = self
            .run(&args)?
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        if replay {
            let range = format!("{to}..{from}");
            let log = [
                "--no-replace-objects",
                "log",
                "-m",
                "--no-renames",
                "--format=",
                "-z",
                "--name-only",
                "--end-of-options",
                &range,
                "--",
            ];
            let tracked = self.run(&[
                "ls-tree",
                "-r",
                "-z",
                "--name-only",
                "--end-of-options",
                &from,
            ])?;
            let tracked: std::collections::BTreeSet<&[u8]> = tracked
                .split(|&b| b == 0)
                .filter(|p| !p.is_empty())
                .collect();
            for path in self.run(&log)?.split(|&b| b == 0) {
                if !path.is_empty() && !tracked.contains(path) && !paths.iter().any(|p| p == path) {
                    paths.push(path.to_vec());
                }
            }
        }
        self.check_paths_free(&paths, what)
    }

    /// [`EngineError::NotAllowed`] if one of the paths `paths`, which the operation `what`
    /// would write, exists in the worktree as untracked or ignored (or a file stands in place of one of
    /// its folders): git would overwrite it.
    pub(super) fn check_paths_free(
        &self,
        paths: &[Vec<u8>],
        what: Message,
    ) -> Result<(), EngineError> {
        let mut hit = Vec::new();
        for path in paths {
            let path = path.as_slice();
            let blocked = std::fs::symlink_metadata(self.workdir.join(os_from_bytes(path))).is_ok()
                || path
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| **b == b'/')
                    .any(|(i, _)| {
                        std::fs::symlink_metadata(self.workdir.join(os_from_bytes(&path[..i])))
                            .is_ok_and(|m| !m.is_dir())
                    });
            if blocked {
                hit.push(format!("`{}`", String::from_utf8_lossy(path)));
            }
        }
        if hit.is_empty() {
            return Ok(());
        }
        Err(EngineError::NotAllowed(msg!(
            "engine-pull-incoming-untracked",
            what = what,
            files = hit.join(", ")
        )))
    }

    /// [`GitRemotes::pull_settings`](project_engine::GitRemotes::pull_settings).
    pub(super) fn read_pull_settings(&self, branch: &str) -> Result<PullSettings, EngineError> {
        let rebase = match self.config_value(&format!("branch.{branch}.rebase"))? {
            Some(value) => Some(value),
            None => self.config_value("pull.rebase")?,
        };
        Ok(PullSettings {
            rebase,
            ff: self.config_value("pull.ff")?,
        })
    }

    /// [`GitRemotes::missing_commits`](project_engine::GitRemotes::missing_commits).
    pub(super) fn commits_missing(
        &self,
        tip: &Oid,
        exclude: &[Oid],
        limit: usize,
    ) -> Result<Vec<Commit>, EngineError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let max = format!("--max-count={limit}");
        let revs: Vec<String> = std::iter::once(tip.to_string())
            .chain(exclude.iter().map(|o| format!("^{o}")))
            .collect();
        let mut args = vec!["--no-replace-objects", "log"];
        args.extend_from_slice(LOG_ARGS);
        args.extend(["--date-order", &max, "--end-of-options"]);
        args.extend(revs.iter().map(String::as_str));
        args.push("--");
        Ok(parse_log(&self.run(&args)?)?)
    }

    /// Removes the backup `name` (only if it still points to `oid`): the pull changed nothing.
    fn drop_backup(&self, name: &str, oid: &Oid) {
        let oid = oid.to_string();
        let _ = self
            .git
            .output(Some(&self.workdir), &["update-ref", "-d", name, &oid]);
    }

    /// [`GitRemotes::pull`](project_engine::GitRemotes::pull).
    pub(super) fn pull_upstream(
        &mut self,
        request: &PullRequest,
    ) -> Result<PullOutcome, EngineError> {
        let full = branch_ref(&request.branch);
        self.check_pull_request(request, &full)?;
        let expected = request.expected.to_string();
        let upstream = request.upstream.to_string();
        let backup = self.create_backup_ref(
            "pull",
            &expected,
            &format!("project: backup of {full} before remote.pull"),
        )?;
        let result = self.integrate(request, &full, &expected, &upstream);
        match result {
            Ok(None) => {}
            Ok(Some(stopped)) => return self.pull_stopped(request, &full, backup, stopped),
            Err(e) => return self.pull_failed(request, &full, &backup, e),
        }
        let after = self
            .ref_value(&full)?
            .ok_or_else(|| EngineError::GitWithReason {
                command: "merge".to_owned(),
                reason: msg!("engine-pull-branch-gone", full = full),
            })?;
        let mut warnings = Vec::new();
        if request.current && self.head_ref_name()?.as_deref() != Some(full.as_str()) {
            warnings.push(msg!("engine-pull-head-moved-after", full = full));
        }
        if !self.is_ancestor(&request.upstream, &after)? {
            warnings.push(msg!(
                "engine-pull-upstream-missing",
                full = full,
                after = after,
                name = request.upstream_name
            ));
        }
        Ok(PullOutcome {
            after,
            backup,
            in_progress: false,
            conflicts: Vec::new(),
            message: String::new(),
            warnings,
        })
    }

    /// The checks before the backup: the state of the repo, the branch where it was seen, HEAD
    /// on it only for the active branch, the incoming paths.
    fn check_pull_request(&self, request: &PullRequest, full: &str) -> Result<(), EngineError> {
        let status = self.status_for_write()?;
        check_pull_state(&status, &request.mode)?;
        let on_head = self.head_ref_name()?.as_deref() == Some(full);
        if on_head != request.current {
            return Err(EngineError::NotAllowed(msg!("engine-pull-head-moved")));
        }
        if !request.current {
            if request.mode != PullMode::FastForward {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-pull-not-current-ff",
                    branch = request.branch.escape_debug()
                )));
            }
            if let Some(path) = self.checked_out_elsewhere(full)? {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-pull-other-worktree",
                    branch = request.branch.escape_debug(),
                    worktree = path.display()
                )));
            }
        }
        self.check_branch_value(full, &request.expected)?;
        if request.current {
            self.check_assume_unchanged(&Some(request.expected), &request.upstream)?;
            let replay = matches!(request.mode, PullMode::Rebase { .. });
            self.check_incoming_paths(
                &request.expected,
                &request.upstream,
                replay,
                msg!("engine-merge-what-pull"),
            )?;
        }
        Ok(())
    }

    /// Stopped merge or rebase: stays in progress, the branch has not moved and the backup
    /// stays. The same checks as a successful merge, as warnings: the operation in progress
    /// must be the upstream's on this branch.
    fn pull_stopped(
        &self,
        request: &PullRequest,
        full: &str,
        backup: String,
        Stopped { conflicts, message }: Stopped,
    ) -> Result<PullOutcome, EngineError> {
        let mut warnings = Vec::new();
        let rebase = matches!(request.mode, PullMode::Rebase { .. });
        let state = self.read_operation_state();
        let on_branch = if rebase {
            state.rebase_branch.as_deref() == Some(full)
        } else {
            self.head_ref_name()?.as_deref() == Some(full)
        };
        if !on_branch {
            warnings.push(msg!("engine-pull-head-moved-merge", full = full));
        }
        let now = self.ref_value(full)?;
        if now != Some(request.expected) {
            warnings.push(msg!(
                "engine-pull-branch-moved",
                full = full,
                now = now.map_or_else(|| msg!("engine-nothing"), |o| Message::plain(o.to_string())),
                expected = request.expected,
                backup = backup
            ));
        }
        // The rebase: what `rebase.continue` needs to conclude it as this pull.
        if rebase {
            let record = RebaseRecord {
                command: "remote.pull".to_owned(),
                branch: request.branch.clone(),
                before: request.expected,
                onto: request.upstream,
                backup: backup.clone(),
                messages: Vec::new(),
                message_commits: Vec::new(),
                applied: 0,
                commits: self.count_unmerged(&request.expected, &[request.upstream])?,
                produces: None,
                upstream: Some(remote_short(&request.upstream_name).to_owned()),
            };
            if let Err(error) = self.write_rebase_record(&record) {
                warnings.push(msg!(
                    "engine-rebase-record-not-written",
                    error = error,
                    backup = backup
                ));
            }
        }
        let incoming = if rebase {
            state.onto.into_iter().collect()
        } else {
            state.incoming
        };
        if incoming != [request.upstream] {
            warnings.push(msg!(
                "engine-pull-merging-other",
                incoming = if incoming.is_empty() {
                    msg!("engine-nothing")
                } else {
                    Message::plain(
                        incoming
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                },
                name = request.upstream_name,
                upstream = request.upstream
            ));
        }
        Ok(PullOutcome {
            after: request.expected,
            backup,
            in_progress: true,
            conflicts,
            message,
            warnings,
        })
    }

    /// Nothing changed: the backup is not needed (removed only if the branch is as it was and
    /// no operation is left in progress, e.g. a failed `rebase --abort`). Always an error.
    fn pull_failed(
        &self,
        request: &PullRequest,
        full: &str,
        backup: &str,
        e: EngineError,
    ) -> Result<PullOutcome, EngineError> {
        let now = self.ref_value(full)?;
        let in_progress = self.operation().is_some();
        if now == Some(request.expected) && !in_progress {
            self.drop_backup(backup, &request.expected);
            return Err(e);
        }
        // Something changed despite the error: the backup stays and the error says so;
        // nothing in the journal (it is not the expected result of the pull).
        let note = if in_progress {
            msg!("engine-pull-note-in-progress", backup = backup)
        } else {
            msg!(
                "engine-pull-note-moved",
                full = full,
                from = request.expected,
                to = match now {
                    Some(o) => msg!("engine-other", text = o.to_string()),
                    None => msg!("engine-remote-head-none"),
                },
                backup = backup
            )
        };
        Err(with_note(e, &note))
    }

    /// The actual integration (see the module): `Ok(None)` if the branch was
    /// moved, `Ok(Some(..))` if the merge is stopped and stays in progress.
    fn integrate(
        &self,
        request: &PullRequest,
        full: &str,
        expected: &str,
        upstream: &str,
    ) -> Result<Option<Stopped>, EngineError> {
        let env: [(&str, &OsStr); 3] = [
            ("GIT_REFLOG_ACTION", OsStr::new(REFLOG_ACTION)),
            ("GIT_EDITOR", OsStr::new(":")),
            ("GIT_SEQUENCE_EDITOR", OsStr::new(":")),
        ];
        let base = ["-c", "core.fsmonitor=false"];
        match (request.mode, request.current) {
            (PullMode::FastForward, false) => {
                let reflog = format!("{REFLOG_ACTION}: fast-forward onto {upstream}");
                let args = ["update-ref", "-m", &reflog, full, upstream, expected];
                let out = self.git.output(Some(&self.workdir), &args)?;
                if !out.status.success() {
                    self.check_branch_value(full, &request.expected)?;
                    return Err(failure(&args, &out));
                }
                Ok(None)
            }
            (PullMode::FastForward, true) => {
                let args = [
                    &base[..],
                    &[
                        "merge",
                        "--ff-only",
                        "--no-autostash",
                        "--no-overwrite-ignore",
                        "--no-edit",
                        "--end-of-options",
                        upstream,
                    ],
                ]
                .concat();
                let out = self.run_input(&args, b"", &env).err();
                match out {
                    None => Ok(None),
                    Some(EngineError::Git { stderr, .. }) => Err(overwrite_error(
                        &stderr,
                        msg!("engine-merge-what-pull"),
                    )
                    .unwrap_or(EngineError::Git {
                        command: "merge".to_owned(),
                        stderr,
                    })),
                    Some(other) => Err(other),
                }
            }
            (PullMode::Merge { no_ff }, _) => {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos());
                let file = TempFile(
                    self.git_dir
                        .join(format!("project-merge-msg-{}-{nanos}", std::process::id())),
                );
                std::fs::write(&file.0, request.message.as_bytes()).map_err(|e| {
                    EngineError::GitWithReason {
                        command: "merge".to_owned(),
                        reason: msg!("engine-merge-message-not-written", error = e),
                    }
                })?;
                let mut args: Vec<&OsStr> = [
                    "-c",
                    "core.fsmonitor=false",
                    "merge",
                    "--no-autostash",
                    "--no-overwrite-ignore",
                    "--no-edit",
                    "--commit",
                    "--no-squash",
                    if no_ff { "--no-ff" } else { "--ff" },
                    "-F",
                ]
                .into_iter()
                .map(OsStr::new)
                .collect();
                // The path as `OsStr`: the git folder may not be UTF-8.
                args.push(file.0.as_os_str());
                args.extend(["--end-of-options", upstream].map(OsStr::new));
                self.run_stopping(&args, &env, PullOperation::Merge)
            }
            (PullMode::Rebase { merges }, _) => {
                let mut args = vec![
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "rebase.updateRefs=false",
                    "rebase",
                    "--no-autostash",
                    "--no-autosquash",
                ];
                if merges {
                    args.push("--rebase-merges");
                }
                args.extend(["--end-of-options", upstream]);
                let args: Vec<&OsStr> = args.into_iter().map(OsStr::new).collect();
                self.run_stopping(&args, &env, PullOperation::Rebase)
            }
        }
    }

    /// Runs a merge or a rebase. A stopped merge or rebase (conflict, hook, signing) stays in
    /// progress: `Ok(Some(..))` with the conflicted files.
    fn run_stopping(
        &self,
        args: &[&OsStr],
        env: &[(&str, &OsStr)],
        operation: PullOperation,
    ) -> Result<Option<Stopped>, EngineError> {
        let out = self.git.output_os_env(Some(&self.workdir), args, env)?;
        if out.status.success() && self.operation().is_none() {
            return Ok(None);
        }
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .trim()
        .to_owned();
        let in_progress = matches!(
            (operation, self.operation()),
            (PullOperation::Merge, Some(Operation::Merge))
                | (PullOperation::Rebase, Some(Operation::Rebase))
        );
        if !in_progress {
            // Nothing started (e.g. an untracked file that would be overwritten): git wrote
            // nothing.
            if let Some(error) = overwrite_error(&text, msg!("engine-merge-what-pull")) {
                return Err(error);
            }
            return Err(EngineError::Git {
                command: operation.to_string(),
                stderr: text,
            });
        }
        // Like `merge.start` and `rebase.start`: the operation stays in progress, it is finished
        // with `merge.continue`/`rebase.continue` or aborted with `merge.abort`/`rebase.abort`
        // (the worktree was clean: it goes back as it was).
        Ok(Some(Stopped {
            conflicts: self.stopped_conflicts(&text),
            message: text,
        }))
    }

    /// The conflicted files of a stopped merge or rebase: those still unmerged in the status and
    /// those git reported as conflicted in the output `text` (even if `rerere.autoUpdate` already
    /// resolved and staged them with a recorded resolution: the operation is still
    /// stopped and must be finished by hand). Also used by `merge.start`.
    pub(super) fn stopped_conflicts(&self, text: &str) -> Vec<RepoPath> {
        let mut files: Vec<RepoPath> = self
            .status_for_write()
            .map(|s| {
                s.entries
                    .iter()
                    .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
                    .map(|e| e.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        add_output_conflicts(&mut files, conflicted_in_output(text));
        files
    }

    /// [`GitRefs::reset_branch`](project_engine::GitRefs::reset_branch).
    pub(super) fn reset_keep(
        &mut self,
        request: &RefRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        request.check(self.operation())?;
        let (name, expected, target) = match (request.refs.as_slice(), &request.head_expected) {
            ([r], HeadState::Branch(b)) if r.name == *b => match (r.expected, r.target) {
                (Some(e), Some(t)) => (r.name.clone(), e, t),
                _ => return Err(invalid(request)),
            },
            _ => return Err(invalid(request)),
        };
        self.check_expected(request)?;
        // Files that `reset --keep` would recreate (in `target`, not in `expected`): nothing must
        // be in their place in the worktree (an ignored file would be overwritten).
        let (from, to) = (expected.to_string(), target.to_string());
        let args = [
            "diff-tree",
            "-r",
            "-z",
            "--name-only",
            "--no-renames",
            "--diff-filter=D",
            "--end-of-options",
            &to,
            &from,
        ];
        let out = self.run(&args)?;
        let present: Vec<String> = out
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .filter(|p| std::fs::symlink_metadata(self.workdir.join(os_from_bytes(p))).is_ok())
            .map(|p| format!("`{}`", String::from_utf8_lossy(p)))
            .collect();
        if !present.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-pull-undo-untracked",
                command = request.command,
                files = present.join(", ")
            )));
        }
        self.check_assume_unchanged(&Some(expected), &target)?;
        let backups = self.protect(request)?;
        let env = [("GIT_REFLOG_ACTION", OsStr::new(&request.reflog))];
        // No `--end-of-options`: `git reset` refuses it before the commit up to git 2.43 at
        // least ("must come before non-option arguments"), and `to` is a full hex id, which
        // never starts with `-`. The final `--`: `to` is a commit even if a file has that name.
        let args = [
            "-c",
            "core.fsmonitor=false",
            "reset",
            "--keep",
            "--quiet",
            &to,
            "--",
        ];
        let out = self
            .git
            .output_with_input_env(Some(&self.workdir), &args, b"", &env)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            if stderr.contains("would be overwritten") || stderr.contains("not uptodate") {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-pull-undo-overwrite-git",
                    command = request.command,
                    name = name,
                    to = to,
                    stderr = stderr.trim()
                )));
            }
            return Err(failure(&args, &out));
        }
        let mut warnings = Vec::new();
        match self.ref_value(&name)? {
            Some(now) if now == target => {}
            now => warnings.push(msg!(
                "engine-ref-moved-after",
                command = request.command,
                name = name,
                now = now.map_or_else(|| msg!("engine-nothing"), |o| Message::plain(o.to_string())),
                target = target
            )),
        }
        Ok(RestoreOutcome { backups, warnings })
    }
}

/// Adds to `files` (the paths from the status, exact bytes) the names git reported as conflicted
/// in the output (text converted with `from_utf8_lossy`): only those that no path of the
/// status already has, comparing the converted text. So a non-UTF-8 name does not appear twice
/// (once with the real bytes, once with the replaced characters).
fn add_output_conflicts(files: &mut Vec<RepoPath>, output: Vec<String>) {
    for file in output {
        if !files.iter().any(|p| p.to_string() == file) {
            files.push(RepoPath::from(file));
        }
    }
}

/// `e` with `note` appended (which says what stayed changed and where the backup is): in git's text
/// or in the refusal reason; other errors become [`EngineError::Git`] with their text.
fn with_note(e: EngineError, note: &Message) -> EngineError {
    match e {
        EngineError::Git { command, stderr } => EngineError::GitWithReason {
            command,
            reason: msg!("engine-note-newline", text = stderr, note = note),
        },
        EngineError::GitWithReason { command, reason } => EngineError::GitWithReason {
            command,
            reason: msg!("engine-note-newline", text = reason, note = note),
        },
        EngineError::NotAllowed(reason) => {
            EngineError::NotAllowed(msg!("engine-note-paren", text = reason, note = note))
        }
        other => EngineError::GitWithReason {
            command: "remote.pull".to_owned(),
            reason: msg!("engine-note-paren", text = other, note = note),
        },
    }
}

/// A pull merge that stopped and stayed in progress: the conflicted files and git's text.
struct Stopped {
    conflicts: Vec<RepoPath>,
    message: String,
}

/// The files the merge/rebase output reports as conflicted: `CONFLICT (...): Merge conflict in
/// <file>` and, with `rerere`, `Resolved '<file>' using previous resolution.` / `Staged '<file>'
/// using previous resolution.`.
pub(super) fn conflicted_in_output(text: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in text.lines() {
        let file = if let Some((_, f)) = line.split_once("Merge conflict in ") {
            Some(f.trim())
        } else {
            ["Resolved '", "Staged '"].iter().find_map(|p| {
                line.strip_prefix(p)?
                    .strip_suffix("' using previous resolution.")
            })
        };
        if let Some(f) = file
            && !files.iter().any(|x| x == f)
        {
            files.push(f.to_owned());
        }
    }
    files
}

fn invalid(request: &RefRestore) -> EngineError {
    EngineError::NotAllowed(msg!(
        "engine-pull-undo-bad-entry",
        command = request.command
    ))
}

/// [`EngineError::NotAllowed`] if git refused because it would overwrite local changes or
/// untracked files (before writing: nothing changed), with the files (lines with the tab).
pub(super) fn overwrite_error(stderr: &str, what: Message) -> Option<EngineError> {
    if !(stderr.contains("would be overwritten by")
        || stderr.contains("would be removed by")
        || stderr.contains("would lose untracked files"))
    {
        return None;
    }
    let files: Vec<String> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix('\t'))
        .map(|f| format!("`{f}`"))
        .collect();
    Some(EngineError::NotAllowed(msg!(
        "engine-pull-overwrite-what",
        what = what,
        files = files.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_utf8_conflict_is_listed_once() {
        let raw = RepoPath::from(&b"caf\xe9.txt"[..]);
        let mut files = vec![raw.clone()];
        let output = conflicted_in_output(&format!(
            "CONFLICT (content): Merge conflict in {raw}\nCONFLICT (content): Merge conflict in \
             b.txt"
        ));
        add_output_conflicts(&mut files, output);
        assert_eq!(files, vec![raw, RepoPath::from("b.txt")]);
    }

    #[test]
    fn the_note_names_the_backup() {
        let e = with_note(
            EngineError::Git {
                command: "rebase".into(),
                stderr: "stopped".into(),
            },
            &msg!(
                "engine-other",
                text = "backup in `refs/project/backup/pull-1`"
            ),
        );
        assert!(e.to_string().contains("refs/project/backup/pull-1"), "{e}");
        let e = with_note(
            EngineError::NotAllowed(msg!("engine-other", text = "no")),
            &msg!("engine-other", text = "note"),
        );
        assert!(
            matches!(&e, EngineError::NotAllowed(m) if m.to_string() == "no (note)"),
            "{e}"
        );
    }
}
