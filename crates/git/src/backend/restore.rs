//! `edit.undo` / `edit.redo` on the [`CliBackend`]: HEAD and refs restored with a
//! `git update-ref --stdin` transaction (§7).
//!
//! In this order, and nothing changes if a step fails:
//!
//! 1. [`RefRestore::check`] with the operation in progress read from `.git` (merge, rebase, ...);
//! 2. HEAD (`symbolic-ref`, or the commit if detached) and the refs (`for-each-ref`, exact name)
//!    compared with the expected values: if something changed from outside,
//!    [`EngineError::UndoConflict`];
//! 3. for each commit that would be abandoned ([`RefRestore::abandoned`]) and that does not stay
//!    reachable from a new value (`merge-base --is-ancestor`) nor from another non-symbolic
//!    ref (`for-each-ref --contains`, backups included; a symbolic ref to the moved
//!    branch does not count): a backup ref
//!    `refs/project/backup/<undo|redo>-<seconds>-<nanoseconds>` created only if it does not exist;
//! 4. a single transaction `update-ref -m <reflog> -z --stdin` with `start`, then for each ref
//!    `update <ref> <new> <old>` / `create <ref> <new>` / `delete <ref> <old>` (git
//!    checks the old value under the ref lock) and, with detached HEAD,
//!    `option no-deref` + `update HEAD <new> <old>`, finally `commit`: all or nothing.
//!    If it fails and the refs are no longer the expected ones, [`EngineError::UndoConflict`];
//!    otherwise git's error (e.g. a `reference-transaction` hook that refuses). Any
//!    backup refs from step 3 stay: they do no harm.
//!
//! Index and worktree are never touched (`update-ref` does not read them). When HEAD points to the
//! moved branch git writes the entry in the HEAD reflog too. Deleting a ref (undo of the
//! first commit) also deletes its reflog, like `git update-ref -d`; the entry in the
//! HEAD reflog stays.
//!
//! Limits (HEAD is not verified inside the transaction):
//!
//! - With HEAD on a branch, whether HEAD still points to that branch is checked at step 2. The
//!   value of the branches instead is checked under lock.
//! - With detached HEAD, `option no-deref` + `update HEAD <new> <old>` checks only the
//!   value: if between step 2 and the transaction another process runs `git checkout main` with
//!   `main` on the expected commit, the transaction succeeds and detaches HEAD again (verified with
//!   git 2.43; `main` does not move, no commit is lost).
//!
//! `symref-verify` (git >= 2.46) is not used: it could only be used for the branch case, but a
//! HEAD check in the same transaction that updates the branch HEAD points to clashes with git's
//! check on multiple updates of HEAD (git adds a reflog-only entry for HEAD by itself),
//! and CI does not have such a recent git to try it. Instead, after the
//! transaction HEAD is re-read and the new entries of the HEAD reflog are counted (at most one,
//! ours): if HEAD is not the requested one or changed in the meantime, the result says so
//! in `warnings`. Without a HEAD reflog only the comparison with the current HEAD remains.
//!
//! The reachability checks (`merge-base`, `for-each-ref --contains`) use
//! `--no-replace-objects`: a replace object could make a commit look reachable that
//! is not for gc.

use super::CliBackend;
use crate::command::{failure, path_from_output};
use project_domain::Oid;
use project_engine::Message;
use project_engine::{EngineError, HeadState, RefMove, RefRestore, RestoreOutcome};
use std::collections::BTreeSet;

impl CliBackend {
    /// [`GitBackend::restore_refs`](project_engine::GitBackend::restore_refs).
    pub(super) fn restore(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError> {
        request.check(self.operation())?;
        // A branch open in another worktree is not deleted (e.g. redo of `branch.delete`).
        for r in &request.refs {
            if r.target.is_none()
                && r.name.starts_with("refs/heads/")
                && let Some(path) = self.checked_out_elsewhere(&r.name)?
            {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-undo-delete-worktree-branch",
                    command = request.command,
                    name = r.name,
                    worktree = path.display()
                )));
            }
        }
        // The HEAD reflog entries before the check: after the transaction there must be at most
        // one more (ours), otherwise HEAD changed in the meantime.
        let head_log_before = self.head_log_len()?;
        self.check_expected(request)?;
        let backups = self.protect(request)?;

        let args = ["update-ref", "-m", &request.reflog, "-z", "--stdin"];
        let out = self
            .git
            .output_with_input(Some(&self.workdir), &args, &transaction(request))?;
        if !out.status.success() {
            // Did someone move a ref between the check and the transaction?
            self.check_expected(request)?;
            return Err(failure(&args, &out));
        }
        let warnings = self.check_head_after(request, head_log_before);
        Ok(RestoreOutcome { backups, warnings })
    }

    /// The HEAD reflog entries: the lines of the worktree's `logs/HEAD` file (the path
    /// is given by `rev-parse --git-path logs/HEAD`). Not `rev-list --walk-reflogs --count HEAD`,
    /// which with HEAD on a branch without commits gives 0 even if the file has lines. With the
    /// reftable format the file does not exist: 0, and only the comparison with the current HEAD remains.
    fn head_log_len(&self) -> Result<usize, EngineError> {
        let out = self.run(&["rev-parse", "--git-path", "logs/HEAD"])?;
        let path = self.workdir.join(path_from_output(&out));
        Ok(match std::fs::read(&path) {
            Ok(content) => content
                .split(|&b| b == b'\n')
                .filter(|l| !l.is_empty())
                .count(),
            Err(_) => 0,
        })
    }

    /// After a successful transaction: HEAD must be the requested one, and in the HEAD reflog there
    /// must be at most one new entry. Otherwise another process changed HEAD
    /// right during the operation: with detached HEAD, for example, a `git checkout main`
    /// done between the check and the transaction (with `main` on the expected commit) is detached
    /// again by `option no-deref` + `update HEAD` (`update-ref` cannot verify that HEAD
    /// is not symbolic). No commit is lost, but it must be said: one warning per problem.
    fn check_head_after(&self, request: &RefRestore, log_before: usize) -> Vec<Message> {
        let mut warnings = Vec::new();
        match self.head_state() {
            Ok(head) if head == request.head_target => {}
            Ok(head) => warnings.push(msg!(
                "engine-undo-head-changed-after",
                command = request.command,
                head = head.to_string(),
                target = request.head_target.to_string()
            )),
            Err(e) => warnings.push(msg!(
                "engine-undo-head-unreadable-after",
                command = request.command,
                error = e
            )),
        }
        if let Ok(after) = self.head_log_len()
            && after > log_before + 1
        {
            warnings.push(msg!(
                "engine-undo-head-reflog-grew",
                command = request.command,
                n = after - log_before,
                target = request.head_target.to_string()
            ));
        }
        warnings
    }

    /// HEAD as the journal records it: the branch it points to (full name, even without
    /// commits) or the commit if detached.
    pub(super) fn head_state(&self) -> Result<HeadState, EngineError> {
        match self.head_ref_name()? {
            Some(name) => Ok(HeadState::Branch(name)),
            None => self
                .resolve("HEAD")?
                .map(HeadState::Detached)
                .ok_or_else(|| EngineError::UnknownRev("HEAD".to_owned())),
        }
    }

    /// The value of the ref `name` (full name, validated by [`RefRestore::check`]), `None` if
    /// it does not exist. `for-each-ref` with the name as pattern also finds the refs under `name/`:
    /// only the one with the exact name is kept.
    pub(super) fn ref_value(&self, name: &str) -> Result<Option<Oid>, EngineError> {
        let args = ["for-each-ref", "--format=%(refname)%00%(objectname)", name];
        let out = self.run(&args)?;
        for line in out.split(|&b| b == b'\n') {
            let mut fields = line.split(|&b| b == 0);
            if fields.next() == Some(name.as_bytes())
                && let Some(hex) = fields.next()
            {
                return Ok(Some(Oid::from_hex(hex)?));
            }
        }
        Ok(None)
    }

    /// [`EngineError::UndoConflict`] if HEAD or a ref do not have the expected value.
    pub(super) fn check_expected(&self, request: &RefRestore) -> Result<(), EngineError> {
        let head = self.head_state()?;
        let conflicts = request.conflicts(&head, |name| self.ref_value(name))?;
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(EngineError::UndoConflict {
                command: request.command,
                refs: conflicts,
            })
        }
    }

    /// Saves in a backup ref every abandoned commit that would not stay reachable, and
    /// returns the names of the refs created. An error stops everything before moving the refs.
    ///
    /// The refs moved by the request and symbolic refs (e.g.
    /// `refs/heads/master -> refs/heads/main`) do not count as protection: `for-each-ref` shows them with the value of the
    /// ref they point to *before* the transaction, and afterwards they would hold nothing. The ref they
    /// point to shows up on its own anyway, if it does not move.
    pub(super) fn protect(&self, request: &RefRestore) -> Result<Vec<String>, EngineError> {
        let targets = request.targets();
        let moved: BTreeSet<&str> = request.refs.iter().map(|r| r.name.as_str()).collect();
        let mut candidates = Vec::new();
        // An annotated tag object (undo of `tag.create`) is always saved: its message and author
        // exist only there, even when the commit it points to stays reachable.
        let mut tags = Vec::new();
        for oid in request.abandoned() {
            if self.is_tag_object(&oid)? {
                tags.push(oid);
                continue;
            }
            let mut kept = false;
            for target in &targets {
                if self.is_ancestor(&oid, target)? {
                    kept = true;
                    break;
                }
            }
            if !kept {
                candidates.push(oid);
            }
        }
        let holders = |oids: &[Oid]| -> Result<bool, EngineError> {
            Ok(self
                .refs_containing_any(oids)?
                .iter()
                .any(|name| !moved.contains(name.as_str())))
        };
        // A single `for-each-ref` for all: usually no other ref contains them.
        let unprotected: Vec<Oid> = if candidates.is_empty() {
            Vec::new()
        } else if !holders(&candidates)? {
            candidates
        } else if candidates.len() == 1 {
            Vec::new()
        } else {
            let mut out = Vec::new();
            for oid in candidates {
                if !holders(std::slice::from_ref(&oid))? {
                    out.push(oid);
                }
            }
            out
        };
        let mut backups = Vec::new();
        for oid in tags.into_iter().chain(unprotected) {
            let name = self.create_backup_ref(
                request.backup_kind(),
                &oid.to_string(),
                &request.backup_reflog(&oid),
            )?;
            backups.push(name);
        }
        Ok(backups)
    }

    /// True if `oid` is an annotated tag object (`cat-file -t`).
    fn is_tag_object(&self, oid: &Oid) -> Result<bool, EngineError> {
        let out = self.run(&["cat-file", "-t", &oid.to_string()])?;
        Ok(out.trim_ascii() == b"tag")
    }

    /// True if commit `a` is reachable from `b` (or is `b`). Without replace objects
    /// (`--no-replace-objects`): counts the real history, the one gc sees.
    pub(super) fn is_ancestor(&self, a: &Oid, b: &Oid) -> Result<bool, EngineError> {
        let (a, b) = (a.to_string(), b.to_string());
        let args = [
            "--no-replace-objects",
            "merge-base",
            "--is-ancestor",
            "--end-of-options",
            &a,
            &b,
        ];
        let out = self.git.output(Some(&self.workdir), &args)?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) if out.stderr.is_empty() => Ok(false),
            _ => Err(failure(&args, &out)),
        }
    }

    /// The non-symbolic refs (branches, remotes, tags, stash, backups, ...) from which at least one of the
    /// commits `ids` is reachable, full names. Without replace objects, like
    /// [`Self::is_ancestor`].
    fn refs_containing_any(&self, ids: &[Oid]) -> Result<Vec<String>, EngineError> {
        let ids: Vec<String> = ids.iter().map(Oid::to_string).collect();
        let mut args = vec![
            "--no-replace-objects",
            "for-each-ref",
            "--format=%(refname)%00%(symref)",
        ];
        for id in &ids {
            args.extend(["--contains", id]);
        }
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
}

/// The input of `update-ref -z --stdin` for `request`: an explicit transaction
/// (`start` ... `commit`) with the old value of every ref.
fn transaction(request: &RefRestore) -> Vec<u8> {
    let mut input = Vec::new();
    let mut push = |fields: &[&str]| {
        for f in fields {
            input.extend_from_slice(f.as_bytes());
            input.push(0);
        }
    };
    push(&["start"]);
    if let (HeadState::Detached(expected), HeadState::Detached(target)) =
        (&request.head_expected, &request.head_target)
    {
        let (expected, target) = (expected.to_string(), target.to_string());
        push(&["option no-deref"]);
        push(&["update HEAD", &target, &expected]);
    }
    for RefMove {
        name,
        expected,
        target,
    } in &request.refs
    {
        let (expected, target) = (
            expected.map(|o| o.to_string()),
            target.map(|o| o.to_string()),
        );
        match (expected.as_deref(), target.as_deref()) {
            (Some(old), Some(new)) => push(&[&format!("update {name}"), new, old]),
            (None, Some(new)) => push(&[&format!("create {name}"), new]),
            (Some(old), None) => push(&[&format!("delete {name}"), old]),
            // Must not exist and stays so: only the check.
            (None, None) => push(&[&format!("verify {name}"), ""]),
        }
    }
    push(&["commit"]);
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_input() {
        let a = Oid::from_bytes([0xaa; Oid::LEN]);
        let b = Oid::from_bytes([0xbb; Oid::LEN]);
        let request = RefRestore {
            command: "edit.undo",
            head_expected: HeadState::Detached(b),
            head_target: HeadState::Detached(a),
            refs: vec![
                RefMove {
                    name: "refs/heads/x".to_owned(),
                    expected: Some(b),
                    target: Some(a),
                },
                RefMove {
                    name: "refs/heads/new".to_owned(),
                    expected: None,
                    target: Some(a),
                },
                RefMove {
                    name: "refs/heads/gone".to_owned(),
                    expected: Some(b),
                    target: None,
                },
            ],
            reflog: "project: undo".to_owned(),
        };
        let text = String::from_utf8(transaction(&request))
            .unwrap()
            .replace('\0', "|");
        let (a, b) = (a.to_string(), b.to_string());
        assert_eq!(
            text,
            format!(
                "start|option no-deref|update HEAD|{a}|{b}|update refs/heads/x|{a}|{b}|\
                 create refs/heads/new|{a}|delete refs/heads/gone|{b}|commit|"
            )
        );
    }
}
