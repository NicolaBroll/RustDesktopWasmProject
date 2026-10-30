//! Stash on the [`CliBackend`] (`stash.list`, `stash.push`, `stash.apply`, `stash.pop`,
//! `stash.drop`, S28), per §5:
//!
//! - list: `git log --walk-reflogs -z --format=tformat:%H%x00%P%x00%ct%x00%gs
//!   --end-of-options refs/stash --` (id, parents, date, reflog message: 4 fields per
//!   record, each terminated by NUL; the fourth NUL is added by `-z`), only if `refs/stash` exists;
//! - push: `git -c core.fsmonitor=false [--literal-pathspecs] stash push [--include-untracked]
//!   --keep-index|--no-keep-index [--message=<message>] [--pathspec-from-file=-
//!   --pathspec-file-nul]`. `git stash` does not read the message from stdin or a file: it goes in
//! a
//!   single argument `--message=<text>` (one line, no control characters: the engine checks
//!   it), which git cannot read as another option. Never `--all` (ignored files stay);
//!   the stash is recognized by the value of `refs/stash` before and after (git exits 0 even when
//!   there is nothing to stash);
//! - apply: `git -c core.fsmonitor=false stash apply [--index] --end-of-options <id>`, with the id
//!   and not `stash@{n}` (the position can change); first the checks of `git/stash.rs` plus the
//!   file system: a file the stash adds, or an untracked one it restores, must not
//!   exist in the worktree (not even ignored, nor a file in place of one of its directories), and
//!   `--assume-unchanged` files that the stash changes are refused (git would overwrite them
//! without
//!   checking);
//! - drop: backup `refs/project/backup/<kind>-...` on the id, recheck that `stash@{n}` is
//!   still that id, `git stash drop --quiet --end-of-options stash@{n}`; the instant between the
//!   recheck and `drop` remains (git has no drop with an expected value), and afterwards the list
//! is compared
//!   with the expected one (another stash dropped by mistake goes in `warnings`);
//! - store (undo of a drop): `git stash store --quiet --message=<message> <id>` (`store` does not
//!   accept `--end-of-options`; the id is hexadecimal and cannot look like an option).

use super::CliBackend;
use super::sync::conflicted_in_output;
use crate::command::{failure, stderr_text};
use project_domain::{EntryKind, Oid, RepoPath};
use project_engine::{
    EngineError, STASH_REF, StashApplied, StashApply, StashDrop, StashDropped, StashEntry,
    StashPush, check_no_operation, check_stash_apply, check_stash_push, find_stash, path_matches,
    stash_blocked, stash_local_changes,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

impl CliBackend {
    /// [`GitStash::stashes`](project_engine::GitStash::stashes).
    pub(super) fn stash_entries(&self) -> Result<Vec<StashEntry>, EngineError> {
        if self.ref_value(STASH_REF)?.is_none() {
            return Ok(Vec::new());
        }
        let args = [
            "log",
            "--walk-reflogs",
            "-z",
            "--no-show-signature",
            "--no-color",
            "--format=tformat:%H%x00%P%x00%ct%x00%gs",
            "--end-of-options",
            STASH_REF,
            "--",
        ];
        let out = self.run(&args)?;
        let bad = || EngineError::GitWithReason {
            command: "log".to_owned(),
            reason: msg!(
                "engine-stash-list-unreadable",
                output = String::from_utf8_lossy(&out).escape_debug()
            ),
        };
        let fields: Vec<&[u8]> = out.split(|&b| b == 0).collect();
        // The last NUL closes the last record: an empty field remains after it.
        let Some((last, fields)) = fields.split_last() else {
            return Err(bad());
        };
        if !last.is_empty() || fields.len() % 4 != 0 {
            return Err(bad());
        }
        let mut list = Vec::new();
        for (index, record) in fields.chunks(4).enumerate() {
            let [id, parents, time, message] = record else {
                return Err(bad());
            };
            let id = Oid::from_hex(id)?;
            let parents = parents
                .split(|&b| b == b' ')
                .filter(|p| !p.is_empty())
                .map(Oid::from_hex)
                .collect::<Result<Vec<_>, _>>()?;
            let time = std::str::from_utf8(time)
                .ok()
                .and_then(|t| t.parse().ok())
                .ok_or_else(bad)?;
            let message = String::from_utf8_lossy(message).into_owned();
            list.push(StashEntry::new(index, id, &parents, time, message).ok_or_else(bad)?);
        }
        Ok(list)
    }

    /// [`GitStash::stash_push`](project_engine::GitStash::stash_push).
    pub(super) fn push_stash(&mut self, request: &StashPush) -> Result<StashEntry, EngineError> {
        // `git stash push` does not see changes to `--assume-unchanged` files (it does not save
        // them) and
        // then resets the worktree to HEAD: it would lose them. Always refused among the
        // chosen files, without looking at the content, before the other checks (the status does
        // not
        // show them: it would look like there is nothing to stash).
        let hidden: Vec<String> = self
            .index_listing()?
            .into_iter()
            .filter(|(p, e)| {
                e.tag.is_ascii_lowercase()
                    && request
                        .paths
                        .as_ref()
                        .is_none_or(|ps| ps.iter().any(|w| path_matches(p, w)))
            })
            .map(|(p, _)| format!("`{p}`"))
            .collect();
        if !hidden.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-stash-push-assume-unchanged",
                files = hidden.join(", ")
            )));
        }
        let status = self.status_for_write()?;
        check_stash_push(&status, request)?;
        self.check_index_unlocked()?;
        let before = self.ref_value(STASH_REF)?;
        let message = request.message.as_ref().map(|m| format!("--message={m}"));
        let mut args = vec!["-c", "core.fsmonitor=false", "stash", "push"];
        if request.include_untracked {
            args.push("--include-untracked");
        }
        args.push(if request.keep_index {
            "--keep-index"
        } else {
            "--no-keep-index"
        });
        args.extend(message.as_deref());
        let out = match &request.paths {
            Some(paths) => self.output_with_paths(&args, paths)?,
            None => self.git.output(Some(&self.workdir), &args)?,
        };
        let after = self.ref_value(STASH_REF)?;
        if !out.status.success() {
            if let Some(id) = after.filter(|a| Some(*a) != before) {
                return Err(EngineError::GitWithReason {
                    command: "stash".to_owned(),
                    reason: msg!(
                        "engine-stash-push-stopped",
                        id = id,
                        stderr = stderr_text(&out)
                    ),
                });
            }
            return Err(failure(&args, &out));
        }
        let Some(id) = after.filter(|a| Some(*a) != before) else {
            let said = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            return Err(EngineError::NotAllowed(msg!(
                "engine-stash-not-created",
                output = if said.is_empty() {
                    stderr_text(&out)
                } else {
                    said
                }
            )));
        };
        self.stash_entries()?
            .into_iter()
            .next()
            .filter(|s| s.id == id)
            .ok_or_else(|| EngineError::GitWithReason {
                command: "stash".to_owned(),
                reason: msg!("engine-stash-not-on-top", id = id),
            })
    }

    /// The tracked files the stash changes (relative to the base, in the saved worktree and
    /// in the saved index) and the untracked ones it restores.
    fn stash_targets(
        &self,
        stash: &StashEntry,
    ) -> Result<(Vec<RepoPath>, Vec<RepoPath>), EngineError> {
        let base = stash.base.to_string();
        let id = stash.id.to_string();
        let index = format!("{id}^2");
        let mut tracked = BTreeSet::new();
        for side in [&id, &index] {
            let args = [
                "diff-tree",
                "-r",
                "-z",
                "--name-only",
                "--no-renames",
                "--end-of-options",
                &base,
                side,
                "--",
            ];
            for p in self
                .run(&args)?
                .split(|&b| b == 0)
                .filter(|p| !p.is_empty())
            {
                tracked.insert(RepoPath::from_bytes(p));
            }
        }
        let mut untracked = Vec::new();
        if stash.untracked {
            // No trailing `--`: `ls-tree` would take it for a path (and would return
            // nothing).
            let tree = format!("{id}^3");
            let args = [
                "ls-tree",
                "-r",
                "-z",
                "--name-only",
                "--full-tree",
                "--end-of-options",
                &tree,
            ];
            for p in self
                .run(&args)?
                .split(|&b| b == 0)
                .filter(|p| !p.is_empty())
            {
                untracked.push(RepoPath::from_bytes(p));
            }
        }
        Ok((tracked.into_iter().collect(), untracked))
    }

    /// The entries (mode, id) of the tree of `rev`, by path (`ls-tree -r -z --full-tree`, without a
    /// trailing `--`).
    fn tree_entries(&self, rev: &str) -> Result<BTreeMap<RepoPath, (u32, Oid)>, EngineError> {
        let args = [
            "ls-tree",
            "-r",
            "-z",
            "--full-tree",
            "--end-of-options",
            rev,
        ];
        let out = self.run(&args)?;
        let mut entries = BTreeMap::new();
        for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            // `<mode> <type> <id>\t<path>`
            let bad = || EngineError::GitWithReason {
                command: "ls-tree".to_owned(),
                reason: msg!(
                    "engine-stash-ls-tree-line",
                    line = String::from_utf8_lossy(record)
                ),
            };
            let tab = record.iter().position(|&b| b == b'\t').ok_or_else(bad)?;
            let fields: Vec<&[u8]> = record[..tab].split(|&b| b == b' ').collect();
            let [mode, _, oid] = fields[..] else {
                return Err(bad());
            };
            let mode = std::str::from_utf8(mode)
                .ok()
                .and_then(|m| u32::from_str_radix(m, 8).ok())
                .ok_or_else(bad)?;
            entries.insert(
                RepoPath::from_bytes(&record[tab + 1..]),
                (mode, Oid::from_hex(oid)?),
            );
        }
        Ok(entries)
    }

    /// True if the worktree has something in place of the file `path` that git would have to
    /// create: the
    /// file itself (even a directory or a link) or, in place of one of its directories, something
    /// that
    /// is not a real directory.
    fn occupied(&self, path: &RepoPath) -> bool {
        let full = self
            .workdir
            .join(crate::command::os_from_bytes(path.as_bytes()));
        if full.symlink_metadata().is_ok() {
            return true;
        }
        let mut dir: &Path = &full;
        while let Some(parent) = dir.parent() {
            if parent == self.workdir || !parent.starts_with(&self.workdir) {
                break;
            }
            if let Ok(meta) = parent.symlink_metadata()
                && !meta.is_dir()
            {
                return true;
            }
            dir = parent;
        }
        false
    }

    /// [`GitStash::stash_apply`](project_engine::GitStash::stash_apply).
    pub(super) fn apply_stash(
        &mut self,
        request: &StashApply,
    ) -> Result<StashApplied, EngineError> {
        let command = request.command;
        check_no_operation(command, msg!("engine-busy-stash-apply"), self.operation())?;
        let list = self.stash_entries()?;
        let stash = find_stash(&list, &request.id).cloned().ok_or_else(|| {
            EngineError::NotAllowed(msg!(
                "engine-stash-gone",
                command = command,
                id = request.id
            ))
        })?;
        let status = self.status_for_write()?;
        check_stash_apply(command, &status)?;
        let (tracked, untracked) = self.stash_targets(&stash)?;
        // The entries of the saved index and worktree: a staged file that already has the same
        // entry in both (e.g. after a push with `keep_index`) does not block.
        let saved_index = self.tree_entries(&format!("{}^2", stash.id))?;
        let saved_worktree = self.tree_entries(&stash.id.to_string())?;
        let in_stash = |path: &RepoPath, mode: u32, oid: Oid| {
            let (i, w) = (saved_index.get(path), saved_worktree.get(path));
            if mode == 0 {
                i.is_none() && w.is_none()
            } else {
                i == Some(&(mode, oid)) && w == Some(&(mode, oid))
            }
        };
        let mut blocked = stash_local_changes(&status, &tracked, &untracked, in_stash);
        let index = self.index_listing()?;
        let in_status: BTreeSet<&RepoPath> = status.entries.iter().map(|e| &e.path).collect();
        // A tracked file the stash adds, if it is in the status, was already seen by
        // `stash_local_changes`; an untracked one to restore must not exist anyway
        // (even if the status shows it, e.g. `D.` with the ignored file in the worktree).
        let added = tracked
            .iter()
            .filter(|p| !index.contains_key(*p) && !in_status.contains(p));
        for p in added.chain(&untracked) {
            if self.occupied(p) && !blocked.contains(&p.to_string()) {
                blocked.push(p.to_string());
            }
        }
        if !blocked.is_empty() {
            return Err(stash_blocked(command, &stash.name, &blocked));
        }
        let hidden: Vec<String> = tracked
            .iter()
            .filter(|p| index.get(*p).is_some_and(|e| e.tag.is_ascii_lowercase()))
            .map(|p| format!("`{p}`"))
            .collect();
        if !hidden.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-stash-apply-assume-unchanged",
                files = hidden.join(", ")
            )));
        }
        self.check_index_unlocked()?;

        let id = stash.id.to_string();
        // `merge.directoryRenames=false`: with directory-rename detection a file
        // the stash adds in `old/` would end up in `new/` (if HEAD renamed the
        // directory) and would overwrite an ignored file there, which the checks above do not see.
        let mut args = vec![
            "-c",
            "core.fsmonitor=false",
            "-c",
            "merge.directoryRenames=false",
            "stash",
            "apply",
        ];
        if request.restore_index {
            args.push("--index");
        }
        args.extend(["--end-of-options", &id]);
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            return Ok(StashApplied::default());
        }
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let after = self.status_for_write()?;
        let mut conflicts: Vec<String> = after
            .entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| e.path.to_string())
            .collect();
        if !conflicts.is_empty() || text.contains("CONFLICT") {
            for f in conflicted_in_output(&text) {
                if !conflicts.contains(&f) {
                    conflicts.push(f);
                }
            }
            return Ok(StashApplied {
                conflicts,
                warnings: Vec::new(),
            });
        }
        if text.contains("onflicts in index") {
            // The key is `INDEX_NOT_RESTORED`: the undo of `stash.push` recognizes it.
            return Err(EngineError::NotAllowed(msg!(
                "engine-stash-index-not-restored",
                name = stash.name
            )));
        }
        // Only if the repo changed between the checks and git: in these cases git has already
        // written
        // part of it.
        let quoted = |files: Vec<String>| {
            files
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if text.contains("already exists") {
            // Git already merged the tracked files, then could not restore the untracked
            // ones: `<file> already exists, no checkout`.
            let files: Vec<String> = text
                .lines()
                .filter_map(|l| l.strip_suffix(" already exists, no checkout"))
                .map(str::to_owned)
                .collect();
            return Err(EngineError::NotAllowed(msg!(
                "engine-stash-apply-untracked-exist",
                command = command,
                name = stash.name,
                files = quoted(files)
            )));
        }
        if text.contains("would be overwritten") {
            // The files are the lines with a tab right after the message (afterwards git also
            // prints
            // a status summary, with other lines starting with a tab).
            let files: Vec<String> = text
                .lines()
                .skip_while(|l| !l.contains("would be overwritten"))
                .skip(1)
                .map_while(|l| l.strip_prefix('\t'))
                .map(str::to_owned)
                .collect();
            return Err(EngineError::NotAllowed(if stash.untracked {
                msg!(
                    "engine-stash-apply-refused-partly",
                    command = command,
                    name = stash.name,
                    files = quoted(files)
                )
            } else {
                msg!(
                    "engine-stash-apply-refused",
                    command = command,
                    name = stash.name,
                    files = quoted(files)
                )
            }));
        }
        Err(failure(&args, &out))
    }

    /// [`GitStash::stash_drop`](project_engine::GitStash::stash_drop).
    pub(super) fn drop_stash(&mut self, request: &StashDrop) -> Result<StashDropped, EngineError> {
        let command = request.command;
        let list = self.stash_entries()?;
        let stash = find_stash(&list, &request.id).cloned().ok_or_else(|| {
            EngineError::NotAllowed(msg!(
                "engine-stash-drop-gone",
                command = command,
                id = request.id
            ))
        })?;
        let id = stash.id.to_string();
        let backup = match request.backup {
            Some(kind) => Some(self.create_backup_ref(
                kind,
                &id,
                &format!("project: backup of {} before {command}", stash.name),
            )?),
            None => None,
        };
        let forget = |this: &Self| {
            if let Some(name) = &backup {
                let _ = this
                    .git
                    .output(Some(&this.workdir), &["update-ref", "-d", name, &id]);
            }
        };
        // Right before: the position is still the same.
        if self.resolve(&stash.name)? != Some(stash.id) {
            forget(self);
            return Err(EngineError::NotAllowed(msg!(
                "engine-stash-drop-list-changed",
                command = command
            )));
        }
        let args = ["stash", "drop", "--quiet", "--end-of-options", &stash.name];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if !out.status.success() {
            forget(self);
            return Err(failure(&args, &out));
        }
        let mut warnings = Vec::new();
        let expected: Vec<Oid> = list
            .iter()
            .filter(|s| s.id != stash.id)
            .map(|s| s.id)
            .collect();
        match self.stash_entries() {
            Ok(now) if now.iter().map(|s| s.id).eq(expected.iter().copied()) => {}
            Ok(_) => warnings.push(match &backup {
                Some(b) => msg!(
                    "engine-stash-list-unexpected",
                    command = command,
                    backup = b
                ),
                None => msg!("engine-stash-list-unexpected-no-backup", command = command),
            }),
            Err(e) => warnings.push(msg!(
                "engine-stash-list-unreadable-after",
                command = command,
                error = e
            )),
        }
        Ok(StashDropped { backup, warnings })
    }

    /// [`GitStash::stash_store`](project_engine::GitStash::stash_store).
    pub(super) fn store_stash(&mut self, id: &Oid, message: &str) -> Result<(), EngineError> {
        let hex = id.to_string();
        let message = format!("--message={message}");
        let args = ["stash", "store", "--quiet", &message, &hex];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        match self.stash_entries()?.first() {
            Some(top) if top.id == *id => Ok(()),
            _ => Err(EngineError::GitWithReason {
                command: "stash".to_owned(),
                reason: msg!("engine-stash-store-not-on-top", id = id),
            }),
        }
    }
}
