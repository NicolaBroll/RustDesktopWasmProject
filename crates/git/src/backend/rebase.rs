//! Interactive rebase on the [`CliBackend`] (`rebase.start`, `rebase.continue`, `rebase.abort`,
//! S32), per §5 and `impl-docs/git/rebase.md`; also the record of a stopped pull with rebase.
//!
//! The plan ([`RebasePlan::todo`](project_domain::RebasePlan::todo)) is written to
//! `<git dir>/project-rebase-todo` and copied over git's todo by the sequence editor
//! (`GIT_SEQUENCE_EDITOR="cp '<path>'"`, the only shell, git's own), then:
//!
//! `git -c core.fsmonitor=false -c rebase.updateRefs=false -c rebase.abbreviateCommands=false -c
//! rebase.missingCommitsCheck=error -c rebase.instructionFormat= -c sequence.editor= rebase -i
//! --no-autostash --no-autosquash --no-rerere-autoupdate --empty=keep --keep-empty
//! --end-of-options <base>`
//!
//! (`rebase.updateRefs=false` and not `--no-update-refs`, which git 2.30 does not know).
//!
//! with `GIT_EDITOR=:` (git keeps its default messages) and `GIT_REFLOG_ACTION=project: <id>`.
//! A `reword`, and a group with a message, is followed by a `break`: at each of these stops the
//! message is applied with `git commit --amend --only --cleanup=whitespace -F -` (hooks and signing according to the
//! configuration), then `git rebase --continue`. Any other stop (conflict, failed hook or
//! signing, an amend that failed) leaves the rebase in progress with the [`RebaseRecord`] in
//! `rebase-merge/project-rebase.json`, which git removes with the folder.
//!
//! First of all the backup `refs/project/backup/rebase-...` of the branch as it was; removed only
//! if git did not start and the branch did not move.

use super::CliBackend;
use super::merge::AbortTarget;
use super::sync::{TempFile, overwrite_error};
use crate::command::os_from_bytes;
use project_domain::{EntryKind, Oid, Operation, RepoPath};
use project_engine::{
    EngineError, MergeAborted, MergeOutcome, Message, REBASE_RECORD, RebaseRecord, RebaseRequest,
    branch_ref, check_rebase_request, check_rebase_state, explicit_squash_messages,
};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::Path;

/// How a run of the rebase ended.
pub(super) enum Driven {
    /// Nothing in progress any more (concluded, or never started): git's text.
    Done(String),
    /// Still in progress: the conflicted files and git's text.
    Stopped(Vec<RepoPath>, String),
}

/// stdout and stderr of a git run, trimmed.
fn text_of(out: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout).trim(),
        String::from_utf8_lossy(&out.stderr).trim()
    )
    .trim()
    .to_owned()
}

/// `cp '<path>'` for `GIT_SEQUENCE_EDITOR`: git runs it with `sh` adding the todo file as an
/// argument; the path is quoted for `sh` (a `'` becomes `'\''`).
fn sequence_editor(path: &Path) -> OsString {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = path.to_string_lossy().replace('\\', "/").into_bytes();
    let mut out = b"cp '".to_vec();
    for &b in &bytes {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    os_from_bytes(&out)
}

impl CliBackend {
    /// The executed instructions of the rebase (`rebase-merge/done`): the last one, the one
    /// before it and how many `break` stops were reached.
    fn rebase_done(&self) -> (Option<String>, Option<String>, usize) {
        let done = std::fs::read(self.git_dir.join("rebase-merge/done")).unwrap_or_default();
        let lines: Vec<String> = done
            .split(|&b| b == b'\n')
            .map(<[u8]>::trim_ascii)
            .filter(|l| !l.is_empty() && !l.starts_with(b"#"))
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .collect();
        let breaks = lines.iter().filter(|l| *l == "break").count();
        let mut rev = lines.into_iter().rev();
        let last = rev.next();
        (last, rev.next(), breaks)
    }

    /// Saves `record` in the folder of the rebase in progress; an error is returned as text.
    pub(super) fn write_rebase_record(&self, record: &RebaseRecord) -> Result<(), String> {
        let dir = self.git_dir.join("rebase-merge");
        if !dir.is_dir() {
            return Err("no `rebase-merge` folder".to_owned());
        }
        std::fs::write(dir.join(REBASE_RECORD), record.to_bytes()).map_err(|e| e.to_string())
    }

    /// The record of the rebase in progress, if the app started it.
    /// Ignored (`None`: no journal entry, the continue warns that the data is missing) if it does
    /// not describe the rebase in progress: `before` must be `orig-head` and `branch` `head-name`
    /// (a record left over, or a rebase restarted from the terminal).
    pub(super) fn read_rebase_record(&self) -> Option<RebaseRecord> {
        let dir = self.git_dir.join("rebase-merge");
        let record = RebaseRecord::from_bytes(&std::fs::read(dir.join(REBASE_RECORD)).ok()?)?;
        let orig = std::fs::read(dir.join("orig-head"))
            .ok()
            .and_then(|b| super::merge::oids(&b).into_iter().next());
        let head_name = std::fs::read(dir.join("head-name")).ok()?;
        let matches = orig == Some(record.before)
            && head_name.trim_ascii() == branch_ref(&record.branch).as_bytes();
        matches.then_some(record)
    }

    /// Runs `git <args>` for the rebase with the editors of the app and the reflog message.
    fn run_rebase(&self, args: &[&OsStr], reflog: &str, editor: Option<&OsStr>) -> String {
        let reflog = format!("project: {reflog}");
        let mut env: Vec<(&str, &OsStr)> = vec![
            ("GIT_REFLOG_ACTION", OsStr::new(&reflog)),
            ("GIT_EDITOR", OsStr::new(":")),
        ];
        if let Some(editor) = editor {
            env.push(("GIT_SEQUENCE_EDITOR", editor));
        }
        match self.git.output_os_env(Some(&self.workdir), args, &env) {
            Ok(out) => text_of(&out),
            Err(e) => e.to_string(),
        }
    }

    /// `git rebase --continue` (the commit stopped on keeps its message: `GIT_EDITOR=:`).
    fn rebase_continue_once(&self, reflog: &str) -> String {
        let args = ["-c", "core.fsmonitor=false", "rebase", "--continue"].map(OsStr::new);
        self.run_rebase(&args, reflog, Some(OsStr::new(":")))
    }

    /// Drives the rebase after a run of git whose text is `text`: at each `break` of the plan
    /// reached and not yet applied, the message (`record.messages`) with `commit --amend`, then
    /// `rebase --continue`; it stops at anything else. A message is applied only if the step
    /// executed before the `break` is the one of `record.message_commits` (otherwise it is skipped
    /// with a warning); a `break` without a message just goes on. Concluded with messages not
    /// applied (e.g. continued from a terminal after a failed amend) → a warning. The record is
    /// saved at every step.
    pub(super) fn drive_rebase(
        &self,
        mut record: Option<&mut RebaseRecord>,
        mut text: String,
        reflog: &str,
        warnings: &mut Vec<Message>,
    ) -> Driven {
        loop {
            if self.operation() != Some(Operation::Rebase) {
                if let Some(r) = record.as_deref()
                    && r.applied < r.messages.len()
                {
                    warnings.push(msg!(
                        "engine-rebase-messages-not-applied",
                        n = r.messages.len() - r.applied,
                        backup = r.backup
                    ));
                }
                return Driven::Done(text);
            }
            let (last, before_last, breaks) = self.rebase_done();
            let conflicts = self.stopped_conflicts(&text);
            let at_break = last.as_deref() == Some("break") && conflicts.is_empty();
            let Some(r) = record
                .as_deref_mut()
                .filter(|r| at_break && r.applied < breaks)
            else {
                if let Some(r) = record.as_deref() {
                    let _ = self.write_rebase_record(r);
                }
                return Driven::Stopped(conflicts, text);
            };
            let n = breaks - 1;
            let step = before_last
                .as_deref()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|o| Oid::from_hex(o.as_bytes()).ok());
            let message = match (r.messages.get(n), r.message_commits.get(n)) {
                (Some(m), Some(c)) if step == Some(*c) => Some(m.clone()),
                (Some(_), Some(c)) => {
                    warnings.push(msg!(
                        "engine-rebase-message-skipped",
                        commit = c.to_string()[..7].to_owned()
                    ));
                    None
                }
                // A `break` that is not one of the plan's: nothing to apply.
                _ => None,
            };
            if let Some(message) = message {
                let reflog_env = format!("project: {reflog}");
                let env = [
                    ("GIT_REFLOG_ACTION", OsStr::new(&reflog_env)),
                    ("GIT_EDITOR", OsStr::new(":")),
                ];
                let args = [
                    "commit",
                    "--amend",
                    "--only",
                    "--quiet",
                    "--cleanup=whitespace",
                    "-F",
                    "-",
                ];
                let amended = self.git.output_with_input_env(
                    Some(&self.workdir),
                    &args,
                    message.as_bytes(),
                    &env,
                );
                let failed = match &amended {
                    Ok(out) if out.status.success() => None,
                    Ok(out) => Some(text_of(out)),
                    Err(e) => Some(e.to_string()),
                };
                if let Some(failed) = failed {
                    let _ = self.write_rebase_record(r);
                    return Driven::Stopped(Vec::new(), failed);
                }
            }
            r.applied = breaks;
            let _ = self.write_rebase_record(r);
            text = self.rebase_continue_once(reflog);
        }
    }

    /// [`GitBackend::rebase`](project_engine::GitBackend::rebase).
    pub(super) fn rebase_onto_base(
        &mut self,
        request: &RebaseRequest,
    ) -> Result<MergeOutcome, EngineError> {
        let status = self.status_for_write()?;
        check_rebase_state(&status)?;
        let full = branch_ref(&request.branch);
        if self.head_ref_name()?.as_deref() != Some(full.as_str()) {
            return Err(EngineError::NotAllowed(msg!("engine-rebase-head-moved")));
        }
        self.check_branch_value(&full, &request.expected)?;
        check_rebase_request(&*self, request)?;
        let base = request.plan.base.to_string();
        let expected = request.expected.to_string();
        self.check_assume_unchanged_between(&base, &expected)?;
        self.check_incoming_paths(
            &request.expected,
            &request.plan.base,
            true,
            msg!("engine-rebase-what"),
        )?;
        self.check_index_unlocked()?;
        // Git's default message of a squash group made explicit (see
        // `explicit_squash_messages`): it no longer depends on `commit.cleanup`.
        let plan = explicit_squash_messages(&request.plan, |c| {
            let c = c.to_string();
            let out = self.run(&["show", "-s", "--format=%B", "--end-of-options", &c])?;
            Ok(String::from_utf8_lossy(&out).into_owned())
        })?;
        let request = &RebaseRequest {
            plan,
            ..request.clone()
        };
        let todo = request.plan.todo();
        let file = TempFile(self.git_dir.join("project-rebase-todo"));
        std::fs::write(&file.0, todo.text.as_bytes()).map_err(|e| EngineError::GitWithReason {
            command: "rebase".to_owned(),
            reason: msg!("engine-rebase-todo-not-written", error = e),
        })?;
        let backup = self.create_backup_ref(
            "rebase",
            &expected,
            &format!("project: backup of {full} before rebase.start"),
        )?;
        let mut record = request.record(&backup);
        let editor = sequence_editor(&file.0);
        let args: Vec<&OsStr> = [
            "-c",
            "core.fsmonitor=false",
            "-c",
            "rebase.updateRefs=false",
            "-c",
            "rebase.abbreviateCommands=false",
            "-c",
            "rebase.missingCommitsCheck=error",
            "-c",
            "rebase.instructionFormat=",
            "-c",
            "sequence.editor=",
            "rebase",
            "-i",
            "--no-autostash",
            "--no-autosquash",
            "--no-rerere-autoupdate",
            "--empty=keep",
            "--keep-empty",
            "--end-of-options",
            &base,
        ]
        .into_iter()
        .map(OsStr::new)
        .collect();
        let text = self.run_rebase(&args, "rebase.start", Some(&editor));
        let started = self.operation() == Some(Operation::Rebase);
        let mut warnings = Vec::new();
        let driven = self.drive_rebase(Some(&mut record), text, "rebase.start", &mut warnings);
        drop(file);
        let now = self.ref_value(&full)?;
        match driven {
            Driven::Stopped(conflicts, message) => {
                // Saved again to report a failure: without it `rebase.continue` would not know
                // the messages still to apply nor the journal entry.
                if let Err(error) = self.write_rebase_record(&record) {
                    warnings.push(msg!(
                        "engine-rebase-record-not-written",
                        error = error,
                        backup = backup
                    ));
                }
                Ok(MergeOutcome {
                    after: now.unwrap_or(request.expected),
                    backup,
                    in_progress: true,
                    conflicts,
                    message,
                    warnings,
                })
            }
            Driven::Done(text) => {
                let moved = now != Some(request.expected);
                if !started && !moved {
                    // Git did not start (e.g. a file that would be overwritten), or changed
                    // nothing: the backup is not needed.
                    self.drop_merge_backup(&backup, &full, &request.expected);
                    if let Some(error) = overwrite_error(&text, msg!("engine-rebase-what")) {
                        return Err(error);
                    }
                    return Err(EngineError::Git {
                        command: "rebase".to_owned(),
                        stderr: text,
                    });
                }
                let after = now.ok_or_else(|| EngineError::GitWithReason {
                    command: "rebase".to_owned(),
                    reason: msg!("engine-rebase-branch-gone", full = full, backup = backup),
                })?;
                if !moved {
                    return Err(EngineError::GitWithReason {
                        command: "rebase".to_owned(),
                        reason: msg!(
                            "engine-rebase-branch-not-moved",
                            full = full,
                            text = text,
                            backup = backup
                        ),
                    });
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
        }
    }

    /// [`GitBackend::rebase_continue`](project_engine::GitBackend::rebase_continue).
    pub(super) fn rebase_go_on(&mut self) -> Result<MergeOutcome, EngineError> {
        let status = self.status_for_write()?;
        project_engine::check_rebasing("rebase.continue", &status)?;
        let conflicts = project_engine::conflicted_paths(&status);
        if !conflicts.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-rebase-continue-conflicts",
                files = project_engine::quoted_list(&conflicts)
            )));
        }
        self.check_index_unlocked()?;
        let state = self.read_operation_state();
        let mut record = state.rebase.clone();
        let branch = state.rebase_branch;
        let (last, _, breaks) = self.rebase_done();
        let pending = record
            .as_ref()
            .is_some_and(|r| last.as_deref() == Some("break") && r.applied < breaks);
        let mut warnings = Vec::new();
        let text = if pending {
            String::new()
        } else {
            // A pick resolved as HEAD was: git drops it and goes on.
            let picking = last
                .as_deref()
                .is_some_and(|l| l.starts_with("pick ") || l.starts_with("p "));
            if picking
                && let Ok(Some(stopped)) = self.resolve("REBASE_HEAD")
                && self.index_matches_head()?
            {
                warnings.push(msg!(
                    "engine-rebase-dropped-empty",
                    commit = stopped.to_string()[..7].to_owned()
                ));
            }
            self.rebase_continue_once("rebase.continue")
        };
        let driven = self.drive_rebase(record.as_mut(), text, "rebase.continue", &mut warnings);
        let after = match &branch {
            Some(full) => self.ref_value(full)?,
            None => self.resolve("HEAD")?,
        };
        let after = after.ok_or_else(|| EngineError::GitWithReason {
            command: "rebase".to_owned(),
            reason: msg!(
                "engine-rebase-branch-gone",
                full = branch.clone().unwrap_or_else(|| "HEAD".to_owned()),
                backup = record
                    .as_ref()
                    .map(|r| r.backup.clone())
                    .unwrap_or_default()
            ),
        })?;
        if let (Driven::Stopped(..), Some(r)) = (&driven, &record)
            && let Err(error) = self.write_rebase_record(r)
        {
            warnings.push(msg!(
                "engine-rebase-record-not-written",
                error = error,
                backup = r.backup
            ));
        }
        let backup = record.map(|r| r.backup).unwrap_or_default();
        Ok(match driven {
            Driven::Stopped(conflicts, message) => MergeOutcome {
                after,
                backup,
                in_progress: true,
                conflicts,
                message,
                warnings,
            },
            Driven::Done(_) => MergeOutcome {
                after,
                backup,
                in_progress: false,
                conflicts: Vec::new(),
                message: String::new(),
                warnings,
            },
        })
    }

    /// [`GitBackend::rebase_abort`](project_engine::GitBackend::rebase_abort).
    pub(super) fn abort_rebase(&mut self) -> Result<MergeAborted, EngineError> {
        let status = self.status_for_write()?;
        project_engine::check_rebasing("rebase.abort", &status)?;
        // `git rebase --abort` is a `reset --hard` to the branch as it was: an untracked or
        // ignored file where that commit has a file would be overwritten. They go in the
        // backup too.
        let dir = if self.git_dir.join("rebase-merge").is_dir() {
            "rebase-merge"
        } else {
            "rebase-apply"
        };
        let orig = std::fs::read(self.git_dir.join(dir).join("orig-head"))
            .ok()
            .and_then(|b| super::merge::oids(&b).into_iter().next());
        let mut extra = Vec::new();
        if let Some(orig) = orig {
            let orig = orig.to_string();
            let tree = self.run(&[
                "ls-tree",
                "-r",
                "-z",
                "--name-only",
                "--end-of-options",
                &orig,
            ])?;
            let index = self.run(&["ls-files", "-z", "--"])?;
            let index: BTreeSet<&[u8]> =
                index.split(|&b| b == 0).filter(|p| !p.is_empty()).collect();
            let on_disk = |p: &[u8]| std::fs::symlink_metadata(self.workdir.join(os_from_bytes(p)));
            for path in tree.split(|&b| b == 0).filter(|p| !p.is_empty()) {
                if index.contains(path) {
                    continue;
                }
                // The file itself, or a file in place of one of its folders (`d` for `d/f`).
                let mut blocking: Vec<&[u8]> = path
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| **b == b'/')
                    .map(|(i, _)| &path[..i])
                    .filter(|dir| !index.contains(dir) && on_disk(dir).is_ok_and(|m| !m.is_dir()))
                    .collect();
                if on_disk(path).is_ok() {
                    blocking.push(path);
                }
                for p in blocking {
                    let p = RepoPath::from(p.to_vec());
                    if !extra.contains(&p) {
                        extra.push(p);
                    }
                }
            }
        }
        // Git itself refuses to overwrite an untracked file (the abort would fail halfway):
        // refused first. An ignored one it overwrites without saying so: it goes in the backup.
        let untracked: Vec<&RepoPath> = extra
            .iter()
            .filter(|p| {
                status
                    .entries
                    .iter()
                    .any(|e| e.path == **p && matches!(e.kind, EntryKind::Untracked))
            })
            .collect();
        if !untracked.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-rebase-abort-untracked-refused",
                files = project_engine::quoted_list(&untracked)
            )));
        }
        let mut aborted = self.abort_operation(
            &status,
            AbortTarget {
                operation: Operation::Rebase,
                git: "rebase",
                command: "rebase.abort",
                head_file: "REBASE_HEAD",
            },
            &extra,
        )?;
        if !extra.is_empty() {
            aborted.warnings.push(msg!(
                "engine-rebase-abort-untracked",
                files = project_engine::quoted_list(&extra)
            ));
        }
        Ok(aborted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_sequence_editor_quotes_the_path_for_sh() {
        let editor = sequence_editor(Path::new("/tmp/it's here/.git/project-rebase-todo"));
        assert_eq!(
            editor,
            OsString::from("cp '/tmp/it'\\''s here/.git/project-rebase-todo'")
        );
    }
}
