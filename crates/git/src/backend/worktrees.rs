//! Linked worktrees on the [`CliBackend`] (`worktree.list`, `worktree.add`, `worktree.remove`,
//! `impl-docs/git/worktrees.md`), following §5. Every option used exists in git 2.30:
//!
//! - list: `git worktree list --porcelain` (records separated by an empty line). Git older than
//!   2.31 prints neither `locked` nor `prunable`: they are read from the administrative folder
//!   `<common dir>/worktrees/<id>` (`locked` file; `gitdir` pointing to a `.git` that no longer
//!   exists). A path is matched with the bytes of `gitdir`; paths with a newline are not
//!   supported (`worktree.add` refuses them);
//! - add: the destination is checked and created as for `repo.clone` (absent or empty folder),
//!   then `git worktree add --no-guess-remote --end-of-options <abs path> <branch>` or `git
//!   worktree add --detach --end-of-options <abs path> <commit id>`; never `--force`, `-b`
//!   (the command creates the branch with `update-ref`), `--lock`. If git fails and the
//!   worktree is not registered, the folder goes back to how it was; if it is registered (a
//!   `post-checkout` hook that failed), success with git's text in `warnings`;
//! - remove: the state is read inside the worktree (`status --porcelain -z
//!   --untracked-files=normal --ignored --ignore-submodules=none`, `ls-files -v -z`, the operation
//!   files of its git directory) after checking that the folder still holds that worktree; a
//!   detached HEAD with commits on no ref is saved in `refs/project/backup/worktree-...`; then
//!   `git worktree remove --end-of-options <path>` (no `--force`: git checks changes, untracked
//!   files and the lock again and refuses submodules). Ignored files are refused here because git
//!   deletes them.

use super::clone::Destination;
use super::{CliBackend, CliOpener};
use crate::command::{failure, path_from_output};
use project_domain::Oid;
use project_engine::{
    EngineError, WORKTREE_BACKUP_KIND, WorktreeAdd, WorktreeAdded, WorktreeContents, WorktreeInfo,
    WorktreeRemove, WorktreeRemoved, WorktreeTarget, removal_needs_confirm, removal_refusal,
    unknown_worktree,
};
use std::fs;
use std::path::{Path, PathBuf};

/// A worktree as listed, with its path as git printed it (bytes).
struct Listed {
    info: WorktreeInfo,
    raw: Vec<u8>,
}

/// The records of `git worktree list --porcelain`, the main worktree first; `locked` and
/// `prunable` only as git printed them (git >= 2.31).
fn parse_porcelain(out: &[u8]) -> Vec<Listed> {
    let mut listed: Vec<Listed> = Vec::new();
    for record in out
        .split(|&b| b == b'\n')
        .collect::<Vec<_>>()
        .split(|l| l.is_empty())
    {
        let mut entry: Option<Listed> = None;
        for line in record {
            let (key, value) = match line.iter().position(|&b| b == b' ') {
                Some(i) => (&line[..i], Some(&line[i + 1..])),
                None => (&line[..], None),
            };
            let text = || value.map(|v| String::from_utf8_lossy(v).into_owned());
            if key == b"worktree" {
                let raw = value.unwrap_or_default().to_vec();
                entry = Some(Listed {
                    info: WorktreeInfo {
                        path: String::from_utf8_lossy(&raw).into_owned(),
                        main: listed.is_empty(),
                        current: false,
                        bare: false,
                        head: None,
                        branch: None,
                        detached: false,
                        locked: false,
                        lock_reason: None,
                        prunable: false,
                    },
                    raw,
                });
                continue;
            }
            let Some(Listed { info, .. }) = entry.as_mut() else {
                continue;
            };
            match key {
                b"HEAD" => {
                    info.head = text()
                        .and_then(|t| t.parse::<Oid>().ok())
                        .filter(|o| o.as_bytes().iter().any(|&b| b != 0));
                }
                b"branch" => info.branch = text(),
                b"detached" => info.detached = true,
                b"bare" => info.bare = true,
                b"locked" => {
                    info.locked = true;
                    info.lock_reason = text().filter(|r| !r.is_empty());
                }
                b"prunable" => info.prunable = true,
                _ => {}
            }
        }
        listed.extend(entry);
    }
    listed
}

fn canonical(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok()
}

/// The non-empty lines of git's output, for `messages`.
fn lines(bytes: &[u8]) -> impl Iterator<Item = String> + '_ {
    bytes
        .split(|&b| b == b'\n')
        .map(|l| String::from_utf8_lossy(l).trim_end().to_owned())
        .filter(|l| !l.is_empty())
}

impl CliBackend {
    /// The common git directory (shared by all worktrees), absolute.
    fn common_dir(&self) -> Result<PathBuf, EngineError> {
        let out = self.run(&["rev-parse", "--git-common-dir"])?;
        Ok(self.workdir.join(path_from_output(&out)))
    }

    /// The worktrees with `locked`, `prunable` and `current` completed.
    fn listed_worktrees(&self) -> Result<Vec<Listed>, EngineError> {
        let out = self.run(&["worktree", "list", "--porcelain"])?;
        let mut listed = parse_porcelain(&out);
        // The administrative folders: `gitdir` (the `.git` file of the worktree) → folder.
        let mut admin: Vec<(Vec<u8>, PathBuf)> = Vec::new();
        if let Ok(entries) = fs::read_dir(self.common_dir()?.join("worktrees")) {
            for entry in entries.flatten() {
                if let Ok(gitdir) = fs::read(entry.path().join("gitdir")) {
                    let gitdir = gitdir.trim_ascii_end().to_vec();
                    admin.push((gitdir, entry.path()));
                }
            }
        }
        let here = canonical(&self.workdir);
        for Listed { info, raw } in &mut listed {
            let path = Path::new(&info.path);
            info.current = here.is_some() && canonical(path) == here;
            if info.main {
                continue;
            }
            let mut dot_git = raw.clone();
            dot_git.extend_from_slice(b"/.git");
            let folder = admin
                .iter()
                .find(|(gitdir, _)| *gitdir == dot_git)
                .map(|(_, folder)| folder);
            if let Some(folder) = folder
                && !info.locked
                && let Ok(reason) = fs::read(folder.join("locked"))
            {
                info.locked = true;
                let reason = String::from_utf8_lossy(reason.trim_ascii()).into_owned();
                info.lock_reason = (!reason.is_empty()).then_some(reason);
            }
            if !info.locked && !info.prunable {
                info.prunable = fs::symlink_metadata(path.join(".git")).is_err();
            }
        }
        Ok(listed)
    }

    /// [`GitWorktrees::worktrees`](project_engine::GitWorktrees::worktrees).
    pub(super) fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, EngineError> {
        Ok(self
            .listed_worktrees()?
            .into_iter()
            .map(|l| l.info)
            .collect())
    }

    /// [`GitWorktrees::add_worktree`](project_engine::GitWorktrees::add_worktree).
    pub(super) fn add_linked_worktree(
        &mut self,
        request: &WorktreeAdd,
    ) -> Result<WorktreeAdded, EngineError> {
        request.check()?;
        let dest = Destination::prepare_in(Some(&self.workdir), &request.path)?;
        let commit;
        let mut args = vec!["worktree", "add"];
        match &request.target {
            WorktreeTarget::Branch(name) => {
                args.extend(["--no-guess-remote", "--end-of-options", dest.as_str(), name]);
            }
            WorktreeTarget::Detached(oid) => {
                commit = oid.to_string();
                args.extend(["--detach", "--end-of-options", dest.as_str(), &commit]);
            }
        }
        let out = match self.git.output(Some(&self.workdir), &args) {
            Ok(out) => out,
            Err(e) => return Err(dest.fail(e)),
        };
        let mut messages: Vec<String> = lines(&out.stdout).chain(lines(&out.stderr)).collect();
        let path = dest.as_str().to_owned();
        let registered = || -> Option<WorktreeInfo> {
            let wanted = canonical(&dest.path)?;
            self.list_worktrees()
                .ok()?
                .into_iter()
                .find(|w| canonical(Path::new(&w.path)).as_ref() == Some(&wanted))
        };
        let mut warnings = Vec::new();
        if !out.status.success() {
            let text = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
            if text.contains("already exists") {
                // Something appeared in the folder after the check: not ours to remove.
                return Err(failure(&args, &out));
            }
            if registered().is_none() {
                return Err(dest.fail(failure(&args, &out)));
            }
            warnings.push(msg!(
                "engine-worktree-add-hook",
                path = path.as_str(),
                message = messages.join("\n")
            ));
            messages.clear();
        }
        // What git checked out must be what was asked (a branch deleted meanwhile would be
        // taken as a commit).
        let expected = match &request.target {
            WorktreeTarget::Branch(name) => (Some(format!("refs/heads/{name}")), None),
            WorktreeTarget::Detached(oid) => (None, Some(*oid)),
        };
        match registered() {
            Some(w) if expected.0.is_some() && w.branch == expected.0 => {}
            Some(w) if expected.1.is_some() && w.detached && w.head == expected.1 => {}
            other => {
                let wanted = match expected {
                    (Some(branch), _) => branch,
                    (None, oid) => oid.map(|o| o.to_string()).unwrap_or_default(),
                };
                let found = match other {
                    None => String::new(),
                    Some(w) => w
                        .branch
                        .unwrap_or_else(|| w.head.map(|o| o.to_string()).unwrap_or_default()),
                };
                warnings.push(msg!(
                    "engine-worktree-add-unexpected",
                    path = path.as_str(),
                    expected = wanted,
                    found = found
                ));
            }
        }
        Ok(WorktreeAdded {
            path,
            messages,
            warnings,
        })
    }

    /// What the linked worktree at `path` holds, read with git run inside it. `None` if its
    /// folder no longer exists; [`EngineError::NotAllowed`] if the folder is there but does not
    /// hold that worktree of this repository.
    fn worktree_contents(&self, path: &Path) -> Result<Option<WorktreeContents>, EngineError> {
        let shown = path.to_string_lossy();
        if fs::symlink_metadata(path).is_err() {
            return Ok(None);
        }
        let not_worktree =
            || EngineError::NotAllowed(msg!("engine-worktree-not-worktree", path = shown.as_ref()));
        if fs::symlink_metadata(path.join(".git")).is_err() {
            return Err(not_worktree());
        }
        let opener = CliOpener {
            git: self.git.clone(),
        };
        let wt = opener.open_dir(path, &shown).map_err(|_| not_worktree())?;
        if canonical(&wt.workdir) != canonical(path)
            || canonical(&wt.common_dir()?) != canonical(&self.common_dir()?)
        {
            return Err(not_worktree());
        }
        let mut contents = WorktreeContents {
            operation: wt.operation(),
            ..Default::default()
        };
        let args = [
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=normal",
            "--ignored",
            "--ignore-submodules=none",
        ];
        let out = wt.run(&args)?;
        let mut records = out.split(|&b| b == 0).filter(|r| !r.is_empty());
        while let Some(record) = records.next() {
            if record.len() < 4 {
                continue;
            }
            let (xy, file) = (
                &record[..2],
                String::from_utf8_lossy(&record[3..]).into_owned(),
            );
            match xy {
                b"??" => contents.untracked.push(file),
                b"!!" => contents.ignored.push(file),
                _ => {
                    // A rename or copy is followed by its original path.
                    if xy.contains(&b'R') || xy.contains(&b'C') {
                        records.next();
                    }
                    contents.changes.push(file);
                }
            }
        }
        let out = wt.run(&["ls-files", "-v", "-z"])?;
        for record in out.split(|&b| b == 0).filter(|r| r.len() > 2) {
            let (tag, file) = (record[0], &record[2..]);
            let present = || {
                fs::symlink_metadata(wt.workdir.join(crate::command::os_from_bytes(file))).is_ok()
            };
            if tag.is_ascii_lowercase() || (tag == b'S' && present()) {
                contents
                    .hidden
                    .push(String::from_utf8_lossy(file).into_owned());
            }
        }
        Ok(Some(contents))
    }

    /// The commit `oid` has commits on no branch, tag or remote branch.
    fn unreferenced(&self, oid: &Oid) -> Result<bool, EngineError> {
        let oid = oid.to_string();
        let out = self.run(&[
            "--no-replace-objects",
            "rev-list",
            "--max-count=1",
            "--not",
            "--branches",
            "--tags",
            "--remotes",
            "--not",
            "--end-of-options",
            &oid,
        ])?;
        Ok(!out.trim_ascii().is_empty())
    }

    /// The listed worktree `path` names: the same bytes, or the same folder.
    fn find_worktree(&self, path: &str) -> Result<Listed, EngineError> {
        let listed = self.listed_worktrees()?;
        let wanted = canonical(&self.workdir.join(path));
        let index = listed
            .iter()
            .position(|l| l.raw == path.as_bytes())
            .or_else(|| {
                let wanted = wanted.as_ref()?;
                listed
                    .iter()
                    .position(|l| canonical(Path::new(&l.info.path)).as_ref() == Some(wanted))
            });
        match index {
            // A path that is not UTF-8 cannot be given back to git as text.
            Some(i) if std::str::from_utf8(&listed[i].raw).is_ok() => {
                Ok(listed.into_iter().nth(i).expect("index found"))
            }
            _ => Err(unknown_worktree(path)),
        }
    }

    /// [`GitWorktrees::remove_worktree`](project_engine::GitWorktrees::remove_worktree).
    pub(super) fn remove_linked_worktree(
        &mut self,
        request: &WorktreeRemove,
    ) -> Result<WorktreeRemoved, EngineError> {
        let Listed { info, .. } = self.find_worktree(&request.path)?;
        if let Some(reason) = removal_refusal(&info, None) {
            return Err(EngineError::NotAllowed(reason));
        }
        let contents = self.worktree_contents(Path::new(&info.path))?;
        if let Some(reason) = removal_refusal(&info, contents.as_ref()) {
            return Err(EngineError::NotAllowed(reason));
        }
        let saved = match info.head {
            Some(head) if info.detached && self.unreferenced(&head)? => Some(head),
            _ => None,
        };
        if !request.confirm {
            return Err(removal_needs_confirm(&info.path, saved.is_some()));
        }
        let backup = match saved {
            Some(head) => Some(self.create_backup_ref(
                WORKTREE_BACKUP_KIND,
                &head.to_string(),
                &format!(
                    "project: backup of detached HEAD {head} of worktree {} before worktree.remove",
                    info.path
                ),
            )?),
            None => None,
        };
        let keep = |error: EngineError| match &backup {
            Some(backup) => EngineError::GitWithReason {
                command: "worktree".to_owned(),
                reason: msg!(
                    "engine-worktree-remove-failed",
                    error = error.to_string(),
                    backup = backup.as_str()
                ),
            },
            None => error,
        };
        // HEAD must still be the one saved (or checked): a commit made there meanwhile would be
        // in no backup.
        match self.find_worktree(&request.path) {
            Ok(now) if now.info.head == info.head && now.info.branch == info.branch => {}
            Ok(_) => {
                return Err(keep(EngineError::NotAllowed(msg!(
                    "engine-worktree-changed",
                    path = info.path.as_str()
                ))));
            }
            Err(e) => return Err(keep(e)),
        }
        let args = ["worktree", "remove", "--end-of-options", info.path.as_str()];
        let out = self.git.output(Some(&self.workdir), &args).map_err(keep)?;
        if !out.status.success() {
            return Err(keep(failure(&args, &out)));
        }
        Ok(WorktreeRemoved {
            path: info.path,
            backup,
            messages: lines(&out.stdout).chain(lines(&out.stderr)).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::parse_porcelain;

    #[test]
    fn parses_the_porcelain_list() {
        let out = b"worktree /r\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n\n\
worktree /w a\nHEAD 2222222222222222222222222222222222222222\ndetached\nlocked why not\n\n\
worktree /x\nHEAD 0000000000000000000000000000000000000000\nbranch refs/heads/new\nprunable gitdir file points to non-existent location\n\n";
        let listed = parse_porcelain(out);
        assert_eq!(listed.len(), 3);
        let [main, w, x] = &listed[..] else {
            unreachable!()
        };
        assert!(main.info.main && !w.info.main);
        assert_eq!(main.info.branch.as_deref(), Some("refs/heads/main"));
        assert_eq!(w.raw, b"/w a");
        assert!(w.info.detached && w.info.locked);
        assert_eq!(w.info.lock_reason.as_deref(), Some("why not"));
        assert_eq!(x.info.head, None);
        assert!(x.info.prunable);
    }
}
