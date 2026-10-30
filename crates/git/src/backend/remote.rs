//! Remotes on the [`CliBackend`] (`remote.fetch`, S25), following §5 and §6:
//!
//! - `git fetch --progress --end-of-options <remote> [refs/heads/<branch>]`: the remote is the name
//!   of a configured remote (checked by the command) and, like the branch, comes after
//!   `--end-of-options`, so a remote named `-x` or `--upload-pack=...` does not become
//!   an option. The branch goes as `refs/heads/<branch>`, without `:` or `+`: no local ref
//!   as destination, and the remote branch is updated with the configured refspec;
//! - `-c protocol.ext.allow=never -c protocol.fd.allow=never` (for push too): a remote
//!   configured by hand with `ext::`/`fd::` does not run commands, whatever `protocol.allow` says;
//! - no `--force`, `--tags`, `--update-head-ok`: tags follow the configuration
//!   (`remote.<name>.tagOpt`, ...); `--prune` only when asked (`FetchRequest::prune`), with
//!   `--no-prune-tags` (a `fetch.pruneTags` would delete local tags) and `--no-auto-gc`; every ref
//!   the fetch removes is then saved in `refs/project/backup/prune-...` (the objects are still
//!   there: nothing runs gc in between): a remote-tracking branch only if no other ref reaches its
//!   commits, any other ref (a refspec toward `refs/heads/`, a mirror) always; without it pruning
//!   follows `fetch.prune`;
//! - stdin closed, `GIT_TERMINAL_PROMPT=0` and no controlling terminal (`setsid`): the fetch
//!   never waits for an answer from the keyboard (see `Git::run_network`); credential helpers,
//!   `ssh-agent` and `SSH_ASKPASS` work as usual; with a `CredentialPrompter` set on the opener
//!   git and ssh ask the user through the app (askpass bridge, S52), and a cancelled question
//!   gives `EngineError::AuthCancelled`;
//! - stderr read while git runs: progress lines (`\r`) go to the `JobContext`, the
//!   other lines into the result or the error.
//!
//! Push (`remote.push`, S26): `git push --porcelain --progress --no-follow-tags
//! --recurse-submodules=check [--set-upstream] [--force-with-lease=refs/heads/<dst>:<oid>]
//! --end-of-options <remote> refs/heads/<branch>:refs/heads/<dst>`, with the same environment as
//! the fetch. The explicit refspec publishes only that branch (`push.default`, `remote.<name>.push`
//! do not count), `--no-follow-tags` neutralizes `push.followTags`, `--recurse-submodules=check`
//! neutralizes a `push.recurseSubmodules=on-demand` that would also publish the submodules (and
//! refuses a push that refers to unpublished submodule commits). Never `--force`, `+refspec`,
//! `--mirror`, `--all`, `--tags`, `--no-verify`: the `pre-push` hooks run. The outcome is read
//! from the stdout of `--porcelain` (`<flag>\t<from>:<to>\t<summary>`).
//!
//! Deletion of a remote branch (`remote.delete_branch`): `git push --porcelain --progress
//! --no-follow-tags --force-with-lease=refs/heads/<b>:<oid> --end-of-options <remote>
//! :refs/heads/<b>`, same environment; never `--delete` with several names, `--prune` or
//! `--force`. The lease makes the deletion fail (`stale info`) if the branch moved or is already gone.
//!
//! Tags (`remote.push_tag`, `remote.delete_tag`, S47): the push of `refs/tags/<n>:refs/tags/<n>`
//! like the branch push, never forced (`[rejected] (already exists)` → `tag_exists`); the remote
//! value read with `git ls-remote --tags --refs`; the deletion like the branch one, with the lease
//! on the value read and the backup fetched straight into `refs/project/backup/push-delete-...`
//! when the object is not here.

use super::CliBackend;
use crate::command::{Segment, Streamed, failure};
use crate::progress::parse_progress;
use project_domain::Oid;
use project_domain::{HEADS_PREFIX, REMOTES_PREFIX};
use project_engine::{
    EngineError, FetchOutcome, FetchRequest, JobContext, PushKind, PushOutcome, PushRejection,
    PushRequest, RemoteBranchDelete, RemoteBranchDeleted, RemoteTagDelete, TagPush, TagPushed,
    tag_ref,
};
use std::process::Output;

impl CliBackend {
    /// The remote names (`git remote`, one per line), in order. A non-UTF-8 name is skipped:
    /// it could not be given in JSON.
    pub(super) fn remote_names(&self) -> Result<Vec<String>, EngineError> {
        let out = self.run(&["remote"])?;
        let mut names: Vec<String> = out
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| std::str::from_utf8(line).ok())
            .map(str::to_owned)
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    pub(super) fn fetch_remote(
        &self,
        request: &FetchRequest,
        job: &JobContext,
    ) -> Result<FetchOutcome, EngineError> {
        let refspec = request
            .branch
            .as_ref()
            .map(|b| format!("{HEADS_PREFIX}{b}"));
        let prune = request.prune && request.branch.is_none();
        let before = if prune {
            self.ref_values("refs/")?
        } else {
            Vec::new()
        };
        let mut args = vec![
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "protocol.fd.allow=never",
            "fetch",
            "--progress",
        ];
        if prune {
            args.extend(["--prune", "--no-prune-tags", "--no-auto-gc"]);
        }
        args.extend(["--end-of-options", &request.remote]);
        args.extend(refspec.as_deref());

        let mut lines = Vec::new();
        let streamed = self.git.run_network(
            &self.workdir,
            &args,
            false,
            &request.remote,
            |Segment { text, newline }| match parse_progress(&text) {
                Some(progress) => job.report(&progress),
                // A line rewritten with `\r` that is not progress (rare) is not kept: the
                // final version arrives with `\n`.
                None if newline && !text.trim().is_empty() => {
                    lines.push(text.trim_end().to_owned())
                }
                None => {}
            },
            &|| job.is_cancelled(),
        )?;
        if streamed.cancelled {
            return Err(EngineError::Cancelled {
                command: "remote.fetch",
            });
        }
        if !streamed.status.success() {
            let stderr = lines.join("\n");
            if let Some(error) = streamed.remote_error(&request.remote, &stderr) {
                return Err(error);
            }
            // `failure` replaces empty text with the exit code.
            return Err(failure(
                &args,
                &Output {
                    status: streamed.status,
                    stdout: Vec::new(),
                    stderr: stderr.into_bytes(),
                },
            ));
        }
        let mut backups = Vec::new();
        if prune {
            let after = self.ref_values("refs/")?;
            for (name, oid) in before.iter().filter(|r| !after.contains(r)) {
                if after.iter().any(|(n, _)| n == name) {
                    continue;
                }
                match self.save_pruned(name, oid) {
                    Ok(Some(backup)) => backups.push(backup),
                    Ok(None) => {}
                    Err(e) => lines.push(format!("warning: no backup of {name} ({oid}): {e}")),
                }
            }
        }
        Ok(FetchOutcome {
            messages: lines,
            backups,
        })
    }

    /// The refs under `prefix` with their value (`for-each-ref`), without the symbolic ones
    /// (`origin/HEAD`) and the app's own (`refs/project/`).
    fn ref_values(&self, prefix: &str) -> Result<Vec<(String, String)>, EngineError> {
        let out = self.run(&[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(symref)",
            prefix,
        ])?;
        Ok(out
            .split(|&b| b == b'\n')
            .filter_map(|line| {
                let mut fields = line.split(|&b| b == 0);
                let name = std::str::from_utf8(fields.next()?).ok()?;
                let oid = std::str::from_utf8(fields.next()?).ok()?;
                let symref = fields.next().unwrap_or_default();
                let own = name.starts_with("refs/project/");
                (symref.is_empty() && !name.is_empty() && !own)
                    .then(|| (name.to_owned(), oid.to_owned()))
            })
            .collect())
    }

    /// The backup of the ref `name` removed by `--prune`, which held `oid`: for a remote-tracking
    /// branch only if some of its commits are no longer reachable from any ref; any other ref always.
    fn save_pruned(&self, name: &str, oid: &str) -> Result<Option<String>, EngineError> {
        let reflog = format!("project: backup of {name} removed by fetch --prune");
        if !name.starts_with(REMOTES_PREFIX) {
            return self.create_backup_ref("prune", oid, &reflog).map(Some);
        }
        let out = self.run(&["rev-list", "--count", oid, "--not", "--all"])?;
        let count = String::from_utf8_lossy(&out);
        if count.trim() == "0" {
            return Ok(None);
        }
        self.create_backup_ref("prune", oid, &reflog).map(Some)
    }
}

impl CliBackend {
    /// [`GitRemotes::push`](project_engine::GitRemotes::push).
    pub(super) fn push_branch(
        &self,
        request: &PushRequest,
        job: &JobContext,
    ) -> Result<PushOutcome, EngineError> {
        let dst = format!("{HEADS_PREFIX}{}", request.remote_branch);
        let refspec = format!("{HEADS_PREFIX}{}:{dst}", request.branch);
        let lease = request.lease.map(|expected| {
            format!(
                "--force-with-lease={dst}:{}",
                expected.map(|o| o.to_string()).unwrap_or_default()
            )
        });
        let mut args = vec![
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "protocol.fd.allow=never",
            "push",
            "--porcelain",
            "--progress",
            "--no-follow-tags",
            "--recurse-submodules=check",
        ];
        if request.set_upstream {
            args.push("--set-upstream");
        }
        args.extend(lease.as_deref());
        args.extend(["--end-of-options", &request.remote, &refspec]);

        // Before a force, the commits the remote would lose (as known by the local
        // remote branch) go into a backup ref.
        let backup = match request.backup {
            Some(oid) => Some(self.create_backup_ref(
                "push",
                &oid.to_string(),
                &format!(
                    "project: backup of {}/{} before a force push",
                    request.remote, request.remote_branch
                ),
            )?),
            None => None,
        };
        let drop_backup = |backup: &Option<String>| {
            if let (Some(name), Some(oid)) = (backup, request.backup) {
                let oid = oid.to_string();
                let _ = self
                    .git
                    .output(Some(&self.workdir), &["update-ref", "-d", name, &oid]);
            }
        };

        let mut lines = Vec::new();
        let streamed = self.git.run_network(
            &self.workdir,
            &args,
            true,
            &request.remote,
            |Segment { text, newline }| match parse_progress(&text) {
                Some(progress) => job.report(&progress),
                None if newline && !text.trim().is_empty() => {
                    lines.push(text.trim_end().to_owned())
                }
                None => {}
            },
            &|| job.is_cancelled(),
        )?;
        if streamed.cancelled {
            // The remote may have already updated the branch: the backup stays.
            return Err(EngineError::Cancelled {
                command: "remote.push",
            });
        }
        let stdout = String::from_utf8_lossy(&streamed.stdout).into_owned();
        let status = porcelain_status(&stdout, &dst);
        let stderr = lines.join("\n");
        if streamed.status.success()
            && let Some((flag, summary)) = &status
        {
            let kind = match flag {
                '*' => PushKind::New,
                '+' => PushKind::Forced,
                '=' => PushKind::UpToDate,
                _ => PushKind::FastForward,
            };
            if kind != PushKind::Forced {
                drop_backup(&backup);
            }
            let mut messages = lines;
            messages.extend(
                stdout
                    .lines()
                    .filter(|l| l.starts_with("To "))
                    .map(str::to_owned),
            );
            return Ok(PushOutcome {
                kind,
                summary: summary.clone(),
                messages,
                backup: if kind == PushKind::Forced {
                    backup
                } else {
                    None
                },
            });
        }
        drop_backup(&backup);
        let reject = |reason, message: String| EngineError::PushRejected {
            remote: request.remote.clone(),
            branch: request.remote_branch.clone(),
            reason,
            message,
        };
        match status {
            Some(('!', summary)) => {
                let reason = rejection(&summary);
                let message = if stderr.is_empty() {
                    summary
                } else {
                    format!("{summary}\n{stderr}")
                };
                Err(reject(reason, message))
            }
            _ => {
                if let Some(error) = streamed.remote_error(&request.remote, &stderr) {
                    return Err(error);
                }
                if stderr.contains("failed to push some refs") {
                    let reason = if stderr.contains("submodule") {
                        PushRejection::Other
                    } else {
                        PushRejection::Hook
                    };
                    return Err(reject(reason, stderr));
                }
                Err(failure(
                    &args,
                    &Output {
                        status: streamed.status,
                        stdout: Vec::new(),
                        stderr: stderr.into_bytes(),
                    },
                ))
            }
        }
    }
}

impl CliBackend {
    /// [`GitRemotes::delete_remote_branch`](project_engine::GitRemotes::delete_remote_branch).
    pub(super) fn delete_branch_on_remote(
        &self,
        request: &RemoteBranchDelete,
        job: &JobContext,
    ) -> Result<RemoteBranchDeleted, EngineError> {
        let expected = request.expected.to_string();
        // Before the deletion, the commit of the branch goes into a backup ref.
        let backup = self.create_backup_ref(
            "push-delete",
            &expected,
            &format!(
                "project: backup of {}/{} before deleting it from the remote",
                request.remote, request.branch
            ),
        )?;
        let target = RemoteDeletion {
            id: "remote.delete_branch",
            remote: &request.remote,
            dst: &format!("{HEADS_PREFIX}{}", request.branch),
            label: &request.branch,
            expected: &expected,
        };
        self.delete_on_remote(&target, backup, job)
    }

    /// [`GitRemotes::remote_tag`](project_engine::GitRemotes::remote_tag): `git ls-remote --tags
    /// --refs --end-of-options <remote> refs/tags/<name>`. `--refs` leaves out the peeled lines
    /// (`^{}`), so the value is the one the remote stores (the tag object of an annotated tag);
    /// the pattern matches the tail of the names, so only the line with exactly `refs/tags/<name>`
    /// counts.
    pub(super) fn tag_value_on_remote(
        &self,
        remote: &str,
        name: &str,
        job: &JobContext,
    ) -> Result<Option<Oid>, EngineError> {
        let full = tag_ref(name);
        let args = [
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "protocol.fd.allow=never",
            "ls-remote",
            "--tags",
            "--refs",
            "--end-of-options",
            remote,
            &full,
        ];
        let (streamed, lines) = self.stream_remote(remote, &args, job)?;
        if streamed.cancelled {
            return Err(EngineError::Cancelled {
                command: "remote.delete_tag",
            });
        }
        if !streamed.status.success() {
            return Err(remote_failure(remote, &args, &streamed, lines));
        }
        for line in streamed.stdout.split(|&b| b == b'\n') {
            let Some(tab) = line.iter().position(|&b| b == b'\t') else {
                continue;
            };
            if &line[tab + 1..] == full.as_bytes() {
                return Ok(Some(Oid::from_hex(&line[..tab])?));
            }
        }
        Ok(None)
    }

    /// [`GitRemotes::push_tag`](project_engine::GitRemotes::push_tag): `git push --porcelain
    /// --progress --no-follow-tags --recurse-submodules=check --end-of-options <remote>
    /// refs/tags/<name>:refs/tags/<name>`, never `--force`.
    pub(super) fn push_tag_to_remote(
        &self,
        request: &TagPush,
        job: &JobContext,
    ) -> Result<TagPushed, EngineError> {
        let dst = tag_ref(&request.name);
        let refspec = format!("{dst}:{dst}");
        let args = [
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "protocol.fd.allow=never",
            "push",
            "--porcelain",
            "--progress",
            "--no-follow-tags",
            "--recurse-submodules=check",
            "--end-of-options",
            &request.remote,
            &refspec,
        ];
        let (streamed, lines) = self.stream_remote(&request.remote, &args, job)?;
        if streamed.cancelled {
            return Err(EngineError::Cancelled {
                command: "remote.push_tag",
            });
        }
        let stdout = String::from_utf8_lossy(&streamed.stdout).into_owned();
        let status = porcelain_status(&stdout, &dst);
        if streamed.status.success()
            && let Some((flag, summary)) = &status
            && *flag != '!'
        {
            let mut messages = lines;
            messages.extend(
                stdout
                    .lines()
                    .filter(|l| l.starts_with("To "))
                    .map(str::to_owned),
            );
            return Ok(TagPushed {
                up_to_date: *flag == '=',
                summary: summary.clone(),
                messages,
            });
        }
        let stderr = lines.join("\n");
        let reject = |reason, message: String| EngineError::PushRejected {
            remote: request.remote.clone(),
            branch: dst.clone(),
            reason,
            message,
        };
        match status {
            Some(('!', summary)) => {
                let reason = if summary.contains("already exists") {
                    PushRejection::TagExists
                } else {
                    rejection(&summary)
                };
                let message = if stderr.is_empty() {
                    summary
                } else {
                    format!("{summary}\n{stderr}")
                };
                Err(reject(reason, message))
            }
            _ => {
                if let Some(error) = streamed.remote_error(&request.remote, &stderr) {
                    return Err(error);
                }
                if stderr.contains("failed to push some refs") {
                    let reason = if stderr.contains("submodule") {
                        PushRejection::Other
                    } else {
                        PushRejection::Hook
                    };
                    return Err(reject(reason, stderr));
                }
                Err(remote_failure(
                    &request.remote,
                    &args,
                    &streamed,
                    Vec::from([stderr]),
                ))
            }
        }
    }

    /// [`GitRemotes::delete_remote_tag`](project_engine::GitRemotes::delete_remote_tag).
    pub(super) fn delete_tag_on_remote(
        &self,
        request: &RemoteTagDelete,
        job: &JobContext,
    ) -> Result<RemoteBranchDeleted, EngineError> {
        const ID: &str = "remote.delete_tag";
        let dst = tag_ref(&request.name);
        let expected = request.expected.to_string();
        let here = self
            .git
            .output(Some(&self.workdir), &["cat-file", "-e", &expected])?
            .status
            .success();
        let backup = if here {
            self.create_backup_ref(
                "push-delete",
                &expected,
                &format!(
                    "project: backup of {dst} on {} before deleting it from the remote",
                    request.remote
                ),
            )?
        } else {
            self.fetch_into_backup(ID, &request.remote, &dst, &expected, job)?
        };
        let target = RemoteDeletion {
            id: ID,
            remote: &request.remote,
            dst: &dst,
            label: &dst,
            expected: &expected,
        };
        self.delete_on_remote(&target, backup, job)
    }

    /// The remote value of `dst` (a tag whose object is not here) fetched straight into a new
    /// backup ref `refs/project/backup/push-delete-...`: `git fetch --no-tags --no-write-fetch-head
    /// --no-recurse-submodules --no-prune --no-auto-gc --refmap= --end-of-options <remote>
    /// <dst>:<backup>`. Only that ref is written: no tags followed, no `FETCH_HEAD`, and `--refmap=`
    /// keeps a configured refspec (e.g. `+refs/tags/*:refs/tags/*`) from also updating the local
    /// tag. The backup must then equal `expected`: otherwise it is removed and
    /// [`EngineError::PushRejected`] with `stale_lease` (the tag moved on the remote).
    fn fetch_into_backup(
        &self,
        id: &'static str,
        remote: &str,
        dst: &str,
        expected: &str,
        job: &JobContext,
    ) -> Result<String, EngineError> {
        let backup = self.free_backup_name("push-delete")?;
        let refspec = format!("{dst}:{backup}");
        let args = [
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "protocol.fd.allow=never",
            "fetch",
            "--progress",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            "--no-prune",
            "--no-auto-gc",
            "--refmap=",
            "--end-of-options",
            remote,
            &refspec,
        ];
        let (streamed, lines) = self.stream_remote(remote, &args, job)?;
        if streamed.cancelled {
            return Err(EngineError::Cancelled { command: id });
        }
        if !streamed.status.success() {
            let stderr = lines.join("\n");
            if stderr
                .to_ascii_lowercase()
                .contains("couldn't find remote ref")
            {
                // Gone from the remote since it was read: nothing to delete any more.
                return Err(EngineError::PushRejected {
                    remote: remote.to_owned(),
                    branch: dst.to_owned(),
                    reason: PushRejection::StaleLease,
                    message: stderr,
                });
            }
            return Err(remote_failure(remote, &args, &streamed, vec![stderr]));
        }
        match self.ref_value(&backup)? {
            Some(oid) if oid.to_string() == expected => Ok(backup),
            now => {
                if let Some(oid) = now {
                    let oid = oid.to_string();
                    let _ = self
                        .git
                        .output(Some(&self.workdir), &["update-ref", "-d", &backup, &oid]);
                }
                Err(EngineError::PushRejected {
                    remote: remote.to_owned(),
                    branch: dst.to_owned(),
                    reason: PushRejection::StaleLease,
                    message: format!(
                        "{dst} on {remote} is now {}, not {expected}",
                        now.map(|o| o.to_string()).unwrap_or_default()
                    ),
                })
            }
        }
    }

    /// A git command that contacts the remote, with the environment of the fetch (see
    /// `Git::run_network`): progress to `job`, the other stderr lines returned; stdout captured.
    fn stream_remote(
        &self,
        remote: &str,
        args: &[&str],
        job: &JobContext,
    ) -> Result<(Streamed, Vec<String>), EngineError> {
        let mut lines = Vec::new();
        let streamed = self.git.run_network(
            &self.workdir,
            args,
            true,
            remote,
            |Segment { text, newline }| match parse_progress(&text) {
                Some(progress) => job.report(&progress),
                None if newline && !text.trim().is_empty() => {
                    lines.push(text.trim_end().to_owned())
                }
                None => {}
            },
            &|| job.is_cancelled(),
        )?;
        Ok((streamed, lines))
    }

    /// The deletion of `dst` from the remote (`remote.delete_branch`, `remote.delete_tag`), after
    /// the backup of `expected` in `backup`: `git push --porcelain --progress --no-follow-tags
    /// --force-with-lease=<dst>:<expected> --end-of-options <remote> :<dst>`. `label` names the
    /// ref in [`EngineError::PushRejected`]. The backup is removed only when nothing was certainly
    /// deleted.
    fn delete_on_remote(
        &self,
        target: &RemoteDeletion<'_>,
        backup: String,
        job: &JobContext,
    ) -> Result<RemoteBranchDeleted, EngineError> {
        let RemoteDeletion {
            id,
            remote,
            dst,
            label,
            expected,
        } = *target;
        let refspec = format!(":{dst}");
        let lease = format!("--force-with-lease={dst}:{expected}");
        let args = [
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "protocol.fd.allow=never",
            "push",
            "--porcelain",
            "--progress",
            "--no-follow-tags",
            &lease,
            "--end-of-options",
            remote,
            &refspec,
        ];
        let drop_backup = || {
            let _ = self.git.output(
                Some(&self.workdir),
                &["update-ref", "-d", &backup, expected],
            );
        };
        let (streamed, lines) = match self.stream_remote(remote, &args, job) {
            Ok(result) => result,
            Err(e) => {
                drop_backup();
                return Err(e);
            }
        };
        if streamed.cancelled {
            // The remote may have already deleted the ref: the backup stays.
            return Err(EngineError::Cancelled { command: id });
        }
        let stdout = String::from_utf8_lossy(&streamed.stdout).into_owned();
        // With several push URLs git pushes to each one: a line per URL.
        let statuses = porcelain_statuses(&stdout, dst);
        let deleted = statuses.iter().find(|(flag, _)| *flag == '-');
        let stderr = lines.join("\n");
        if let Some((_, summary)) = deleted {
            // Deleted at least on one URL: a success, the backup stays; if another URL refused
            // (or git failed after), `incomplete` says so.
            let incomplete = !streamed.status.success() || statuses.iter().any(|(f, _)| *f != '-');
            let mut messages = lines;
            messages.extend(
                stdout
                    .lines()
                    .filter(|l| l.starts_with("To ") || (incomplete && l.starts_with('!')))
                    .map(str::to_owned),
            );
            return Ok(RemoteBranchDeleted {
                summary: summary.clone(),
                messages,
                backup,
                incomplete,
            });
        }
        let status = statuses.into_iter().next();
        let reject = |reason, message: String| EngineError::PushRejected {
            remote: remote.to_owned(),
            branch: label.to_owned(),
            reason,
            message,
        };
        // The backup is removed only when nothing was certainly deleted: git rejected the ref
        // everywhere, a `pre-push` hook stopped it before sending, the credentials were refused.
        // In every other case (connection lost, unknown error) the remote may have deleted the
        // ref already, and the backup stays.
        match status {
            Some(('!', summary)) => {
                drop_backup();
                let message = if stderr.is_empty() {
                    summary.clone()
                } else {
                    format!("{summary}\n{stderr}")
                };
                Err(reject(rejection(&summary), message))
            }
            _ => {
                if let Some(error) = streamed.remote_error(remote, &stderr) {
                    if matches!(
                        error,
                        EngineError::Auth { .. } | EngineError::AuthCancelled { .. }
                    ) {
                        drop_backup();
                    }
                    return Err(error);
                }
                if status.is_none() && stderr.contains("failed to push some refs") {
                    drop_backup();
                    return Err(reject(PushRejection::Hook, stderr));
                }
                Err(failure(
                    &args,
                    &Output {
                        status: streamed.status,
                        stdout: Vec::new(),
                        stderr: stderr.into_bytes(),
                    },
                ))
            }
        }
    }
}

/// What [`CliBackend::delete_on_remote`] deletes: `dst` (full name) on `remote`, if it is still
/// `expected` there; `label` names it in the errors.
#[derive(Clone, Copy)]
struct RemoteDeletion<'a> {
    id: &'static str,
    remote: &'a str,
    dst: &'a str,
    label: &'a str,
    expected: &'a str,
}

/// The error of a failed command that contacts `remote`: [`EngineError::Auth`] /
/// [`EngineError::RemoteUnreachable`] from the text, otherwise git's error with the stderr `lines`.
fn remote_failure(
    remote: &str,
    args: &[&str],
    streamed: &Streamed,
    lines: Vec<String>,
) -> EngineError {
    let stderr = lines.join("\n");
    if let Some(error) = streamed.remote_error(remote, &stderr) {
        return error;
    }
    failure(
        args,
        &Output {
            status: streamed.status,
            stdout: Vec::new(),
            stderr: stderr.into_bytes(),
        },
    )
}

/// The `git push --porcelain` line for the remote ref `dst`: the flag (` ` fast-forward,
/// `+` forced, `*` new, `=` up to date, `!` rejected, `-` deleted) and the summary.
fn porcelain_status(stdout: &str, dst: &str) -> Option<(char, String)> {
    stdout.lines().find_map(|line| {
        let mut fields = line.splitn(3, '\t');
        let flag = fields.next()?;
        let refs = fields.next()?;
        let summary = fields.next().unwrap_or_default();
        let (_, to) = refs.rsplit_once(':')?;
        let mut chars = flag.chars();
        let flag = chars.next()?;
        (to == dst && chars.next().is_none()).then(|| (flag, summary.to_owned()))
    })
}

/// All the `git push --porcelain` lines for the remote ref `dst`, one per push URL, in order.
fn porcelain_statuses(stdout: &str, dst: &str) -> Vec<(char, String)> {
    stdout
        .split_inclusive('\n')
        .filter_map(|line| porcelain_status(line, dst))
        .collect()
}

/// The reason for a rejection from the summary of `--porcelain` (`[rejected] (non-fast-forward)`,
/// `[rejected] (stale info)`, `[remote rejected] (pre-receive hook declined)`, ...).
fn rejection(summary: &str) -> PushRejection {
    if summary.contains("non-fast-forward") || summary.contains("fetch first") {
        PushRejection::NonFastForward
    } else if summary.contains("stale info") {
        PushRejection::StaleLease
    } else if summary.contains("remote rejected") || summary.contains("remote failure") {
        PushRejection::RemoteRejected
    } else {
        PushRejection::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_lines() {
        let out = "To ../r.git\n \trefs/heads/main:refs/heads/main\t7291e58..b4f1e36\nDone\n";
        assert_eq!(
            porcelain_status(out, "refs/heads/main"),
            Some((' ', "7291e58..b4f1e36".to_owned()))
        );
        assert_eq!(porcelain_status(out, "refs/heads/x"), None);
        let out = "To r\n!\trefs/heads/a:refs/heads/a\t[rejected] (non-fast-forward)\nDone\n";
        let (flag, summary) = porcelain_status(out, "refs/heads/a").unwrap();
        assert_eq!(
            (flag, rejection(&summary)),
            ('!', PushRejection::NonFastForward)
        );
        assert_eq!(
            rejection("[rejected] (fetch first)"),
            PushRejection::NonFastForward
        );
        assert_eq!(
            rejection("[rejected] (stale info)"),
            PushRejection::StaleLease
        );
        assert_eq!(
            rejection("[remote rejected] (pre-receive hook declined)"),
            PushRejection::RemoteRejected
        );
        let out = "*\trefs/heads/-x:refs/heads/-x\t[new branch]\n";
        assert_eq!(
            porcelain_status(out, "refs/heads/-x"),
            Some(('*', "[new branch]".to_owned()))
        );
    }
}
