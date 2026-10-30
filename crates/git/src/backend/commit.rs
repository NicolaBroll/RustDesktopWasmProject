//! `commit.create` on the [`CliBackend`]: `git commit` with the message on stdin.
//!
//! Hooks (`pre-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit`, `post-rewrite`) and
//! signing (`commit.gpgSign`, `gpg.format`, `user.signingKey`) follow the user's
//! configuration: never `--no-verify` nor `--no-gpg-sign`. Message cleanup is that of
//! `commit.cleanup` (without configuration, with `-F` git uses `whitespace`: `#` lines
//! stay). No editor: `--no-edit` and `GIT_EDITOR=:` (`:` is the value with which git starts
//! no editor), so neither `core.editor` nor `commit.verbose`, `commit.template` or
//! `commit.status` can open an editor that would wait for a terminal that is not there (with `-F`
//! the template is ignored anyway).
//!
//! A hook or signing can be slow: the command stays synchronous like the others (§6) and
//! stdout and stderr are read together while stdin is written from another thread, so a
//! hook that prints a lot does not block git. There is no timeout: a graphical gpg pinentry (or the
//! confirmation of an ssh key) legitimately waits for the user; without a terminal or graphical
//! pinentry gpg fails immediately (git's stdin is a pipe, `GIT_TERMINAL_PROMPT=0`). Instead these
//! can wait: a hook that leaves background processes with stdout/stderr open (the
//! command finishes when they close them); an ssh signature with a passphrase and no agent or a
//! hook that
//! reads `/dev/tty`, on the controlling terminal if there is one (CLI or MCP started from a
//! terminal).
//!
//! After a successful commit every problem is [`EngineError::PostCommit`], with the commit created
//! (if HEAD is still that one) and the backup: the commit must not be repeated.

use super::CliBackend;
use crate::command::{failure, stderr_text};
use project_domain::{Commit, LOG_ARGS, Oid, parse_log};
use project_domain::{HEADS_PREFIX, REMOTES_PREFIX, TAGS_PREFIX};
use project_engine::{
    CommitOutcome, CommitRequest, EngineError, Message, amend_impact, check_amend_published,
    check_commit_state,
};
use std::ffi::OsStr;

impl CliBackend {
    /// [`GitIndex::commit`](project_engine::GitIndex::commit).
    pub(super) fn create_commit(
        &mut self,
        request: &CommitRequest,
    ) -> Result<CommitOutcome, EngineError> {
        request.check_message()?;
        let status = self.status_for_write()?;
        let previous = self.resolve("HEAD")?;
        if previous.is_none() {
            // HEAD that does not resolve: fine only for a branch without commits, not for a
            // broken ref (it would be a root commit in place of the history).
            self.index_source()?;
        }
        check_commit_state(&status, previous.is_some(), request)?;
        let branch = self.head_ref_name()?;

        let mut backup = None;
        let mut also_on = Vec::new();
        if request.amend
            && let Some(old) = previous
        {
            let (published, diverging) =
                amend_impact(&self.refs_containing(&old)?, branch.as_deref());
            check_amend_published(request, &published)?;
            also_on = diverging;
            // The rewritten commit stays in the reflog, but the reflog expires: a ref keeps it.
            let reflog = format!("project: backup of {old} before commit.create with amend");
            backup = Some(self.create_backup_ref("amend", &old.to_string(), &reflog)?);
        }

        let mut args = vec![
            // The message comes from JSON, so it is always UTF-8: a different `i18n.commitEncoding`
            // would label it with the wrong encoding.
            "-c",
            "i18n.commitEncoding=UTF-8",
            "commit",
            "--no-edit",
            "-F",
            "-",
        ];
        if request.amend {
            args.push("--amend");
        }
        let env = [("GIT_EDITOR", OsStr::new(":"))];
        let out = self.git.output_with_input_env(
            Some(&self.workdir),
            &args,
            request.message.as_bytes(),
            &env,
        )?;
        if !out.status.success() {
            let mut err = commit_failure(&args, &out);
            if let (Some(name), Some(old)) = (&backup, previous) {
                if self.resolve("HEAD")? == Some(old) {
                    // Nothing committed and HEAD is still the commit read before: the backup is not
                    // needed. `update-ref -d <ref> <old>` removes the ref only if it still points
                    // to that commit (only the instant between reading HEAD and this remains, in
                    // which another process could move HEAD: the backup would vanish but the
                    // commit would still be in the reflog).
                    let old = old.to_string();
                    let _ = self
                        .git
                        .output(Some(&self.workdir), &["update-ref", "-d", name, &old]);
                } else {
                    // HEAD changed (a hook or another process): the backup stays.
                    let note = msg!("engine-commit-backup-kept", old = old, name = name);
                    err = match err {
                        EngineError::Git { command, stderr } => EngineError::GitWithReason {
                            command,
                            reason: msg!("engine-commit-backup-note", text = stderr, note = note),
                        },
                        EngineError::GitWithReason { command, reason } => {
                            EngineError::GitWithReason {
                                command,
                                reason: msg!(
                                    "engine-commit-backup-note",
                                    text = reason,
                                    note = note
                                ),
                            }
                        }
                        other => other,
                    };
                }
            }
            return Err(err);
        }

        // From here the commit is done: every error is `PostCommit`, with the commit and the
        // backup.
        let post = |commit: Option<Oid>, reason: Message| EngineError::PostCommit {
            commit,
            backup: backup.clone(),
            reason,
        };
        let head = self
            .resolve("HEAD")
            .map_err(|e| post(None, msg!("engine-commit-head-unreadable", error = e)))?
            .ok_or_else(|| post(None, msg!("engine-commit-head-no-commit")))?;
        if request.amend && Some(head) == previous {
            // Same tree, message, author and dates (e.g. two identical `amend`s in the same
            // second): git rewrote the identical commit. Nothing changed, the backup points
            // to the same commit and is removed.
            if let Some(name) = &backup {
                let old = head.to_string();
                let _ = self
                    .git
                    .output(Some(&self.workdir), &["update-ref", "-d", name, &old]);
            }
            let commit = self
                .read_commit(&head)
                .map_err(|e| post(Some(head), msg!("engine-commit-unreadable", error = e)))?;
            return Ok(CommitOutcome {
                commit,
                previous,
                branch,
                backup: None,
                also_on,
            });
        }
        // If HEAD is not the commit created, that commit is not known: `commit` stays `None` and
        // the reason names the current HEAD.
        self.check_moved_from(branch.as_deref(), head, previous)
            .map_err(|reason| post(None, reason))?;
        let commit = self
            .read_commit(&head)
            .map_err(|e| post(Some(head), msg!("engine-commit-unreadable", error = e)))?;
        // Extra check on the parents (also works without a reflog).
        let expected_parents = match (request.amend, previous) {
            (true, Some(old)) => {
                self.read_commit(&old)
                    .map_err(|e| {
                        post(
                            Some(head),
                            msg!("engine-commit-rewritten-unreadable", error = e),
                        )
                    })?
                    .parents
            }
            (_, previous) => previous.into_iter().collect(),
        };
        let parents_match = if request.amend {
            commit.parents == expected_parents
        } else {
            // A merge commit has `MERGE_HEAD` in addition.
            commit.parents.first() == expected_parents.first()
        };
        if Some(head) == previous || !parents_match {
            return Err(post(None, moved_text(previous, head)));
        }
        Ok(CommitOutcome {
            commit,
            previous,
            branch,
            backup,
            also_on,
        })
    }

    /// After the commit: the last reflog entry of the updated ref (`branch`, or HEAD if
    /// detached) must go from `previous` to `head`. This also detects another process that
    /// moved HEAD between reading `previous` and `git commit` (git would have rewritten that
    /// commit, not the one read and saved in the backup). The old value is `<ref>@{1}`, the new
    /// `<ref>@{0}`; without a previous value (first commit) `@{1}` does not resolve.
    ///
    /// Limit: without a reflog (`core.logAllRefUpdates=false` and no log file for the ref)
    /// it cannot be verified and only the check on the parents remains.
    fn check_moved_from(
        &self,
        branch: Option<&str>,
        head: Oid,
        previous: Option<Oid>,
    ) -> Result<(), Message> {
        let name = branch.unwrap_or("HEAD");
        let at = |n: u8| {
            self.resolve(&format!("{name}@{{{n}}}"))
                .map_err(|e| msg!("engine-commit-reflog-unreadable", name = name, error = e))
        };
        let Some(newest) = at(0)? else {
            // No reflog: nothing to verify.
            return Ok(());
        };
        if newest != head || at(1)? != previous {
            return Err(moved_text(previous, head));
        }
        Ok(())
    }

    /// The full name of the ref HEAD points to (e.g. `refs/heads/main`), `None` if HEAD is
    /// detached.
    pub(super) fn head_ref_name(&self) -> Result<Option<String>, EngineError> {
        let args = ["symbolic-ref", "--quiet", "HEAD"];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            let name = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
            return Ok(Some(String::from_utf8_lossy(name).into_owned()));
        }
        // Code 1 without messages: detached HEAD.
        if out.status.code() == Some(1) && out.stderr.is_empty() {
            Ok(None)
        } else {
            Err(failure(&args, &out))
        }
    }

    /// The local branches, remote branches and tags (without symbolic refs like `origin/HEAD`) from
    /// which the commit `id` is reachable, full names.
    fn refs_containing(&self, id: &Oid) -> Result<Vec<String>, EngineError> {
        let id = id.to_string();
        let args = [
            "for-each-ref",
            "--format=%(refname)%00%(symref)",
            "--contains",
            &id,
            HEADS_PREFIX,
            REMOTES_PREFIX,
            TAGS_PREFIX,
        ];
        let out = self.run(&args)?;
        Ok(out
            .split(|&b| b == b'\n')
            .filter_map(|line| {
                let mut fields = line.split(|&b| b == 0);
                let name = fields.next().filter(|n| !n.is_empty())?;
                let symref = fields.next().unwrap_or_default();
                symref
                    .is_empty()
                    .then(|| String::from_utf8_lossy(name).into_owned())
            })
            .collect())
    }

    /// The commit `id`, read as in `repo.log`.
    fn read_commit(&self, id: &Oid) -> Result<Commit, EngineError> {
        let hex = id.to_string();
        let mut args = vec!["log"];
        args.extend_from_slice(LOG_ARGS);
        args.extend(["--max-count=1", "--end-of-options", &hex, "--"]);
        parse_log(&self.run(&args)?)?
            .into_iter()
            .next()
            .ok_or_else(|| EngineError::GitWithReason {
                command: "log".to_owned(),
                reason: msg!("engine-git-no-commit", hex = hex),
            })
    }
}

/// The reason of [`EngineError::PostCommit`] when HEAD is not the expected commit.
fn moved_text(previous: Option<Oid>, head: Oid) -> Message {
    match previous {
        Some(previous) => msg!("engine-commit-head-moved", head = head, previous = previous),
        None => msg!("engine-commit-head-moved-unborn", head = head),
    }
}

/// The error of a failed `git commit`: locked index → [`EngineError::IndexLocked`];
/// message emptied by the `commit.cleanup` cleanup → [`EngineError::InvalidParams`];
/// otherwise [`EngineError::Git`] with stderr and stdout of git **and of the hooks** (git sends
/// the hooks' output to stderr), so the caller sees why a hook or the signing
/// refused the commit.
fn commit_failure(args: &[&str], out: &std::process::Output) -> EngineError {
    let locked = failure(args, out);
    if matches!(locked, EngineError::IndexLocked { .. }) {
        return locked;
    }
    let stderr = stderr_text(out);
    if stderr.contains("Aborting commit due to empty commit message") {
        return EngineError::empty_commit_message();
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let text = match (stderr.is_empty(), stdout.is_empty()) {
        (true, true) => {
            return EngineError::GitWithReason {
                command: "commit".to_owned(),
                reason: msg!(
                    "engine-commit-silent-failure",
                    status = out.status.to_string()
                ),
            };
        }
        (false, true) => stderr,
        (true, false) => stdout,
        (false, false) => format!("{stderr}\n{stdout}"),
    };
    EngineError::Git {
        command: "commit".to_owned(),
        stderr: text,
    }
}
