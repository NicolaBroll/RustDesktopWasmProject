//! Local branches on the [`CliBackend`] (`branch.create`, `branch.checkout`, `branch.delete`,
//! `branch.rename` and their undo/redo, S24), following §5:
//!
//! - names reach git only as `refs/heads/<name>` (`update-ref`) or after
//!   `--end-of-options` (`switch`, `branch -m`): a branch named
//!   `-x` never becomes an option;
//! - a new name is also checked by git (`check-ref-format --branch`, which must return
//!   the identical name: no expansions like `@{-1}`);
//! - no ref is overwritten: creation with `update-ref <ref> <id> ""` (only if it does not exist),
//!   deletion with `update-ref -d <ref> <id>` (only if it still holds `<id>`), rename with
//!   `git branch -m` without `-M` (refuses if the new name exists);
//! - checkout is `git switch` without `--force`, `--discard-changes` or `--merge`, with
//!   `--no-overwrite-ignore` (git by itself overwrites ignored files without saying so) and
//!   `--no-guess` (no branch created from a remote), and `core.fsmonitor=false` (a stale fsmonitor
//!   would make a modified file look clean). Git checks everything before
//!   writing: if a file would be overwritten nothing changes.
//!
//! Known limits: a modified `--assume-unchanged` file would be overwritten by git without
//! warning; the checkout refuses it if the file differs between HEAD and the destination. A branch on
//! which a rebase or bisect is stopped in another worktree (HEAD detached there) does not count as "open"
//! and can be deleted: the commit stays in the backup.

use super::CliBackend;
use crate::command::failure;
use project_domain::{EntryKind, Oid, RepoPath};
use project_engine::Message;
use project_engine::{
    BranchDeleted, ConfigEntry, EngineError, HEADS_PREFIX, HeadState, RestoreOutcome, branch_ref,
    check_new_branch_name, check_no_operation,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

impl CliBackend {
    /// `git check-ref-format --branch <name>` must accept `name` and return it identical (with
    /// `@{-1}` or `@` git would give the name of another branch). First [`check_new_branch_name`].
    fn check_new_name(&self, id: &'static str, name: &str) -> Result<(), EngineError> {
        check_new_branch_name(id, name)?;
        let out = self.git.output(
            Some(&self.workdir),
            // `--branch <name>` takes the argument literally (`check-ref-format` has no
            // `--end-of-options`); a leading `-` is already refused by `check_new_branch_name`.
            &["check-ref-format", "--branch", name],
        )?;
        let printed = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
        if out.status.success() && printed == name.as_bytes() {
            Ok(())
        } else {
            Err(EngineError::InvalidParams {
                id,
                reason: msg!("engine-branch-git-rejects-name", name = name.escape_debug()),
            })
        }
    }

    /// The ref `full` points to if it is a symbolic ref, also to a ref that does not exist
    /// (`git symbolic-ref -q`: exit 1 for a ref that is not symbolic or does not exist).
    fn symbolic_target(&self, full: &str) -> Result<Option<String>, EngineError> {
        let args = ["symbolic-ref", "-q", full];
        let out = self.git.output(Some(&self.workdir), &args)?;
        match out.status.code() {
            Some(0) => Ok(Some(
                String::from_utf8_lossy(&out.stdout).trim_end().to_owned(),
            )),
            Some(1) => Ok(None),
            _ => Err(failure(&args, &out)),
        }
    }

    /// The worktree (other than this one) in which the branch `full` is the HEAD branch, if any
    /// (`git worktree list --porcelain`).
    pub(super) fn checked_out_elsewhere(&self, full: &str) -> Result<Option<PathBuf>, EngineError> {
        let out = self.run(&["worktree", "list", "--porcelain"])?;
        let here = canonical(&self.workdir);
        let mut current: Option<PathBuf> = None;
        for line in out.split(|&b| b == b'\n') {
            if let Some(path) = line.strip_prefix(b"worktree ") {
                current = Some(PathBuf::from(String::from_utf8_lossy(path).into_owned()));
            } else if let Some(name) = line.strip_prefix(b"branch ")
                && name == full.as_bytes()
                && let Some(path) = &current
                && canonical(path) != here
            {
                return Ok(Some(path.clone()));
            }
        }
        Ok(None)
    }

    /// [`EngineError::NotAllowed`] if `full` is not `expected`.
    pub(super) fn check_branch_value(&self, full: &str, expected: &Oid) -> Result<(), EngineError> {
        match self.ref_value(full)? {
            Some(now) if now == *expected => Ok(()),
            Some(now) => Err(EngineError::NotAllowed(msg!(
                "engine-branch-changed",
                full = full,
                expected = expected,
                now = now
            ))),
            None => Err(EngineError::NotAllowed(msg!(
                "engine-branch-missing",
                full = full
            ))),
        }
    }

    /// [`GitRead::unmerged_count`](project_engine::GitRead::unmerged_count):
    /// `rev-list --count <tip> ^<into>...`, without replace objects (counts the real history).
    pub(super) fn count_unmerged(&self, tip: &Oid, into: &[Oid]) -> Result<usize, EngineError> {
        let revs: Vec<String> = std::iter::once(tip.to_string())
            .chain(into.iter().map(|o| format!("^{o}")))
            .collect();
        let mut args = vec![
            "--no-replace-objects",
            "rev-list",
            "--count",
            "--end-of-options",
        ];
        args.extend(revs.iter().map(String::as_str));
        let out = self.run(&args)?;
        let text = String::from_utf8_lossy(&out);
        text.trim().parse().map_err(|_| EngineError::GitWithReason {
            command: "rev-list".to_owned(),
            reason: msg!("engine-git-unexpected-output", output = text.trim()),
        })
    }

    /// [`GitRefs::create_branch`](project_engine::GitRefs::create_branch).
    pub(super) fn create_local_branch(
        &mut self,
        name: &str,
        target: &Oid,
    ) -> Result<(), EngineError> {
        self.check_new_name("branch.create", name)?;
        let full = branch_ref(name);
        let oid = target.to_string();
        let reflog = format!("project: branch.create {name} at {oid}");
        // Older git (2.43 does) overwrites a symbolic ref that points nowhere even with `--no-deref`
        // and an empty old value: refused here first.
        if let Some(target) = self.symbolic_target(&full)? {
            return Err(EngineError::NotAllowed(msg!(
                "engine-branch-name-taken-git",
                name = name.escape_debug(),
                stderr = format!("`{full}` is a symbolic ref to `{target}`")
            )));
        }
        // Empty old value: the ref is created only if it does not exist. `--no-deref`: a symbolic
        // ref `refs/heads/<name>` (even to a ref that does not exist) is not followed, so
        // exactly `refs/heads/<name>` is created or refused.
        let args = ["update-ref", "--no-deref", "-m", &reflog, &full, &oid, ""];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if self.ref_value(&full)?.is_some()
            || stderr.contains("already exists")
            || stderr.contains("exists; cannot create")
            || stderr.contains("reference already exists")
        {
            // It already exists, is a symbolic ref, or a ref contains it or is contained by it
            // (`a` for `a/b`): git's message says which.
            return Err(EngineError::NotAllowed(msg!(
                "engine-branch-name-taken-git",
                name = name.escape_debug(),
                stderr = stderr.trim()
            )));
        }
        Err(failure(&args, &out))
    }

    /// [`GitRefs::checkout`](project_engine::GitRefs::checkout) with `git switch`.
    pub(super) fn switch_to(&mut self, target: &HeadState) -> Result<RestoreOutcome, EngineError> {
        check_no_operation(
            "branch.checkout",
            msg!("engine-busy-checkout"),
            self.operation(),
        )?;
        let status = self.status_for_write()?;
        let unmerged: Vec<String> = status
            .entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| e.path.to_string())
            .collect();
        if !unmerged.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-branch-checkout-conflicts",
                files = unmerged.join(", ")
            )));
        }
        let (label, oid, detach) = match target {
            HeadState::Branch(full) => {
                let name = full.strip_prefix(HEADS_PREFIX).ok_or_else(|| {
                    EngineError::NotAllowed(msg!("engine-branch-not-local", full = full))
                })?;
                let oid = self.ref_value(full)?.ok_or_else(|| {
                    EngineError::NotAllowed(msg!(
                        "engine-branch-no-local",
                        name = name.escape_debug()
                    ))
                })?;
                (name.to_owned(), oid, false)
            }
            HeadState::Detached(oid) => {
                let found = self.resolve(&oid.to_string())?;
                if found != Some(*oid) {
                    return Err(EngineError::UnknownRev(oid.to_string()));
                }
                (oid.to_string(), *oid, true)
            }
        };
        self.check_assume_unchanged(&status.head.target(), &oid)?;

        let mut args = vec![
            "-c",
            "core.fsmonitor=false",
            "switch",
            "--no-guess",
            "--no-overwrite-ignore",
        ];
        if detach {
            args.push("--detach");
        }
        args.extend(["--end-of-options", &label]);
        let out = self.git.output(Some(&self.workdir), &args)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            // Git lists the files with a tab in front: local changes or untracked files that the
            // checkout would overwrite or remove, folders with untracked files that
            // would become files, and (with `submodule.recurse`) the submodule files that their
            // checkout would overwrite ("would be overwritten by merge").
            if stderr.contains("would be overwritten by")
                || stderr.contains("would be removed by")
                || stderr.contains("would lose untracked files")
                || stderr.contains("would be lost")
            {
                let files = stderr
                    .lines()
                    .filter_map(|l| l.strip_prefix('\t'))
                    .map(str::to_owned)
                    .collect();
                return Err(EngineError::CheckoutConflict {
                    target: label,
                    files,
                    stderr: stderr.trim().to_owned(),
                });
            }
            return Err(failure(&args, &out));
        }
        let mut warnings = Vec::new();
        match self.head_state() {
            Ok(head) if head == *target => {}
            Ok(head) => warnings.push(msg!(
                "engine-branch-checkout-head-moved",
                head = head.to_string(),
                target = target.to_string()
            )),
            Err(e) => warnings.push(msg!("engine-branch-checkout-head-unreadable", error = e)),
        }
        Ok(RestoreOutcome {
            backups: Vec::new(),
            warnings,
        })
    }

    /// Git does not check `--assume-unchanged` files (lowercase letter in `ls-files -v`): a
    /// checkout would overwrite their changes without warning. Refuses if one of them
    /// differs between HEAD (`head`, `None` without commits) and `target`.
    pub(super) fn check_assume_unchanged(
        &self,
        head: &Option<Oid>,
        target: &Oid,
    ) -> Result<(), EngineError> {
        let from = head.map_or_else(|| self.empty_tree.to_owned(), |h| h.to_string());
        self.check_assume_unchanged_between(&from, &target.to_string())
    }

    /// Like [`Self::check_assume_unchanged`], between any two trees or commits (`from` can
    /// be the empty tree).
    pub(super) fn check_assume_unchanged_between(
        &self,
        from: &str,
        to: &str,
    ) -> Result<(), EngineError> {
        let listing = self.index_listing()?;
        let hidden: BTreeSet<&RepoPath> = listing
            .iter()
            .filter(|(_, e)| e.tag.is_ascii_lowercase())
            .map(|(p, _)| p)
            .collect();
        if hidden.is_empty() {
            return Ok(());
        }
        let args = [
            "diff-tree",
            "-r",
            "-z",
            "--name-only",
            "--no-renames",
            "--end-of-options",
            from,
            to,
        ];
        let out = self.run(&args)?;
        let hit: Vec<String> = out
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| RepoPath::from(p.to_vec()))
            .filter(|p| hidden.contains(p))
            .map(|p| format!("`{p}`"))
            .collect();
        if hit.is_empty() {
            Ok(())
        } else {
            Err(EngineError::NotAllowed(msg!(
                "engine-branch-assume-unchanged",
                files = hit.join(", ")
            )))
        }
    }

    /// [`GitRefs::delete_branch`](project_engine::GitRefs::delete_branch).
    pub(super) fn delete_local_branch(
        &mut self,
        name: &str,
        expected: &Oid,
    ) -> Result<BranchDeleted, EngineError> {
        let full = branch_ref(name);
        if self.head_ref_name()?.as_deref() == Some(full.as_str()) {
            return Err(EngineError::NotAllowed(msg!(
                "engine-branch-delete-current",
                name = name.escape_debug()
            )));
        }
        if let Some(path) = self.checked_out_elsewhere(&full)? {
            return Err(EngineError::NotAllowed(msg!(
                "engine-branch-delete-other-worktree",
                name = name.escape_debug(),
                worktree = path.display()
            )));
        }
        self.check_branch_value(&full, expected)?;
        // Before touching anything: the configuration to remove, saved for undo.
        let (config, mut warnings) = self.read_branch_config(name)?;
        let oid = expected.to_string();
        let backup = self.create_backup_ref(
            "delete",
            &oid,
            &format!("project: backup of {full} before branch.delete"),
        )?;
        let reflog = format!("project: branch.delete {name} (was {oid})");
        let args = ["update-ref", "--no-deref", "-m", &reflog, "-d", &full, &oid];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if !out.status.success() {
            // Nothing deleted: the backup is not needed (removed only if it still points there).
            let _ = self
                .git
                .output(Some(&self.workdir), &["update-ref", "-d", &backup, &oid]);
            self.check_branch_value(&full, expected)?;
            return Err(failure(&args, &out));
        }
        // Like `git branch -d`: the upstream and the rest of `branch.<name>.*` go too.
        if !config.is_empty()
            && let Err(e) = self.remove_branch_config(name)
        {
            warnings.push(msg!(
                "engine-branch-config-not-removed-delete",
                name = name,
                error = e
            ));
        }
        Ok(BranchDeleted {
            backup,
            config,
            warnings,
        })
    }

    /// [`GitRefs::rename_branch`](project_engine::GitRefs::rename_branch) with `git
    /// branch -m` (never `-M`).
    pub(super) fn rename_local_branch(
        &mut self,
        from: &str,
        to: &str,
        expected: &Oid,
    ) -> Result<RestoreOutcome, EngineError> {
        self.check_new_name("branch.rename", to)?;
        let (old, new) = (branch_ref(from), branch_ref(to));
        self.check_branch_value(&old, expected)?;
        if self.ref_value(&new)?.is_some() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-branch-exists",
                name = to.escape_debug()
            )));
        }
        let args = ["branch", "-m", "--end-of-options", from, to];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        let mut warnings = Vec::new();
        match self.ref_value(&new)? {
            Some(now) if now == *expected => {}
            now => warnings.push(msg!(
                "engine-branch-rename-moved",
                new = new,
                now = now.map_or_else(|| msg!("engine-nothing"), |o| Message::plain(o.to_string())),
                expected = expected
            )),
        }
        Ok(RestoreOutcome {
            backups: Vec::new(),
            warnings,
        })
    }
}

impl CliBackend {
    /// The `branch.<name>.*` entries of the repo configuration (`git config --local -z
    /// --get-regexp`, only the repo file: no global configuration nor `include`),
    /// in file order, and the warnings for non-UTF-8 values (not saved).
    fn read_branch_config(
        &self,
        name: &str,
    ) -> Result<(Vec<ConfigEntry>, Vec<Message>), EngineError> {
        let pattern = format!("^branch\\.{}\\.[^.]+$", regex_escape(name));
        let args = [
            "config",
            "--local",
            "-z",
            "--get-regexp",
            "--end-of-options",
            &pattern,
        ];
        let out = self.git.output(Some(&self.workdir), &args)?;
        // Code 1 without messages: no entries.
        if out.status.code() == Some(1) && out.stderr.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        for record in out.stdout.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            let (key, value) = match record.iter().position(|&b| b == b'\n') {
                Some(i) => (&record[..i], Some(&record[i + 1..])),
                None => (record, None),
            };
            let key = String::from_utf8_lossy(key).into_owned();
            match value.map(|v| String::from_utf8(v.to_vec())) {
                Some(Ok(v)) => entries.push(ConfigEntry {
                    key,
                    value: Some(v),
                }),
                None => entries.push(ConfigEntry { key, value: None }),
                Some(Err(_)) => warnings.push(msg!("engine-branch-config-not-utf8", key = key)),
            }
        }
        Ok((entries, warnings))
    }

    /// `git config --local --remove-section branch.<name>`; no section is not an error.
    fn remove_branch_config(&self, name: &str) -> Result<(), EngineError> {
        let section = format!("branch.{name}");
        let args = [
            "config",
            "--local",
            "--remove-section",
            "--end-of-options",
            &section,
        ];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() || String::from_utf8_lossy(&out.stderr).contains("no such section")
        {
            Ok(())
        } else {
            Err(failure(&args, &out))
        }
    }

    /// [`GitRefs::restore_branch_config`](project_engine::GitRefs::restore_branch_config).
    pub(super) fn set_branch_config(
        &mut self,
        name: &str,
        entries: &[ConfigEntry],
        remove: bool,
    ) -> Vec<Message> {
        let current = match self.read_branch_config(name) {
            Ok((current, _)) => current,
            Err(e) => {
                return vec![msg!(
                    "engine-branch-config-unreadable",
                    name = name,
                    error = e
                )];
            }
        };
        if remove {
            if current.is_empty() {
                return Vec::new();
            }
            if current != entries {
                return vec![msg!("engine-branch-config-changed", name = name)];
            }
            return match self.remove_branch_config(name) {
                Ok(()) => Vec::new(),
                Err(e) => vec![msg!(
                    "engine-branch-config-not-removed",
                    name = name,
                    error = e
                )],
            };
        }
        if entries.is_empty() {
            return Vec::new();
        }
        if !current.is_empty() {
            return vec![msg!(
                "engine-branch-config-exists-saved",
                name = name,
                entries = describe(entries)
            )];
        }
        for entry in entries {
            // A key without a value means `true`.
            let value = entry.value.as_deref().unwrap_or("true");
            let args = [
                "config",
                "--local",
                "--add",
                "--end-of-options",
                &entry.key,
                value,
            ];
            match self.git.output(Some(&self.workdir), &args) {
                Ok(out) if out.status.success() => {}
                Ok(out) => {
                    return vec![msg!(
                        "engine-branch-config-partly",
                        name = name,
                        error = failure(&args, &out),
                        entries = describe(entries)
                    )];
                }
                Err(e) => return vec![msg!("engine-branch-config-not-written", error = e)],
            }
        }
        Vec::new()
    }
}

/// `key=value, ...` for the warnings.
fn describe(entries: &[ConfigEntry]) -> String {
    entries
        .iter()
        .map(|e| format!("{}={}", e.key, e.value.as_deref().unwrap_or("true")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `name` with the special characters of extended regular expressions preceded by `\`.
pub(super) fn regex_escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if r"\.^$|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The path without symbolic links, if it resolves (to compare worktrees).
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
