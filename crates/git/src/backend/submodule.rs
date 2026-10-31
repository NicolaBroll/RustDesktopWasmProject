//! `submodule.list` and `submodule.update` on the [`CliBackend`] (`impl-docs/git/submodules.md`):
//!
//! - the submodules are the gitlinks of the index (`ls-files -s -z`, mode `160000`), named by
//!   the worktree's `.gitmodules`, with the URL of the repo configuration (set by init) or of
//!   `.gitmodules`;
//! - each checked-out submodule is read with git run **inside it**, only after checking that
//!   the repo found there is the submodule's own (its top level is the submodule folder): an
//!   empty folder would otherwise lead git up to the superproject;
//! - the update runs `git [-c protocol.file.allow=user] --literal-pathspecs submodule update
//!   [--init] --no-recommend-shallow --checkout --progress -- <paths>` only for the submodules
//!   [`plan_update`] lets through, never with `--force`, `--remote`, `--merge`, `--rebase`;
//!   `--checkout` overrides `submodule.<name>.update` (merge, rebase, a command). The detached HEAD
//!   of each one is saved first in a backup ref inside it; if a backup fails nothing runs.

use super::{CliBackend, CliOpener};
use crate::command::{Git, Segment, failure, os_from_bytes, path_from_output};
use crate::progress::parse_progress;
use project_domain::{EntryKind, Head, Oid, RepoPath, parse_status};
use project_engine::{
    EngineError, JobContext, SUBMODULE_BACKUP_KIND, SubmoduleInfo, SubmoduleMoved,
    SubmoduleRefusal, SubmoduleRefusalReason, SubmoduleState, SubmoduleUpdate, SubmoduleUpdated,
    mask_url, not_at_recorded, plan_update, refusal_message, submodule_state, update_refusal,
};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Output;

/// Mode of a gitlink (a submodule) in the index.
const GITLINK: &[u8] = b"160000";

/// `-c protocol.file.allow=user` unless the user's configuration (any scope, as seen from `dir`)
/// sets `protocol.file.allow` or `protocol.allow`: then their choice stands, as in a terminal.
/// `user` is git's default since 2.38.1; passing it makes older versions (whose default was
/// `always`) refuse a submodule URL that is a local path (CVE-2022-39253): `git submodule` runs
/// its clones with `GIT_PROTOCOL_FROM_USER=0`.
pub(super) fn file_protocol_args(git: &Git, dir: &Path) -> &'static [&'static str] {
    let set = |key: &str| {
        git.output(Some(dir), &["config", "--get", key])
            .is_ok_and(|out| out.status.success())
    };
    if set("protocol.file.allow") || set("protocol.allow") {
        &[]
    } else {
        &["-c", "protocol.file.allow=user"]
    }
}

/// Reads `git config -z` output (`<key>\n<value>\0`) into `(name, variable, value)` for the
/// `submodule.<name>.<variable>` keys.
fn submodule_keys(out: &[u8]) -> Vec<(String, String, Vec<u8>)> {
    out.split(|&b| b == 0)
        .filter(|r| !r.is_empty())
        .filter_map(|record| {
            let (key, value) = match record.iter().position(|&b| b == b'\n') {
                Some(nl) => (&record[..nl], record[nl + 1..].to_vec()),
                None => (record, Vec::new()),
            };
            let key = String::from_utf8_lossy(key);
            let rest = key.strip_prefix("submodule.")?;
            let (name, variable) = rest.rsplit_once('.')?;
            Some((name.to_owned(), variable.to_owned(), value))
        })
        .collect()
}

impl CliBackend {
    /// The gitlinks of the index: path and the recorded commit (`None` if conflicted).
    fn gitlinks(&self) -> Result<BTreeMap<RepoPath, Option<Oid>>, EngineError> {
        let args = ["ls-files", "-s", "-z", "--"];
        let out = self.run(&args)?;
        let mut links = BTreeMap::new();
        for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            // `<mode> <id> <stage>\t<path>`
            let bad = || super::failure_text(&args, record);
            let tab = record.iter().position(|&b| b == b'\t').ok_or_else(bad)?;
            let (head, path) = (&record[..tab], &record[tab + 1..]);
            let fields: Vec<&[u8]> = head.split(|&b| b == b' ').collect();
            let [mode, oid, stage] = fields[..] else {
                return Err(bad());
            };
            if mode != GITLINK {
                continue;
            }
            let recorded = (stage == b"0").then(|| Oid::from_hex(oid)).transpose()?;
            let entry = links.entry(RepoPath::from_bytes(path)).or_insert(recorded);
            if recorded.is_none() {
                *entry = None;
            }
        }
        Ok(links)
    }

    /// `git config -z [--file .gitmodules] --get-regexp <regexp>`, empty if nothing matches
    /// or the file does not exist.
    fn config_regexp(&self, file: Option<&str>, regexp: &str) -> Result<Vec<u8>, EngineError> {
        let mut args = vec!["config", "-z"];
        if let Some(file) = file {
            args.extend(["--file", file]);
        }
        args.extend(["--get-regexp", regexp]);
        let out = self.git.output(Some(&self.workdir), &args)?;
        match out.status.code() {
            Some(0) => Ok(out.stdout),
            // No match (also for a missing file).
            Some(1) => Ok(Vec::new()),
            _ => Err(failure(&args, &out)),
        }
    }

    /// The folder of the submodule at `path`.
    fn submodule_dir(&self, path: &RepoPath) -> PathBuf {
        self.workdir.join(os_from_bytes(path.as_bytes()))
    }

    /// The submodule at `path` as a repo of its own, `None` if it is not checked out: no `.git`
    /// in its folder, or the repo git finds there is not rooted in it.
    pub(super) fn submodule_repo(&self, path: &RepoPath) -> Option<CliBackend> {
        let dir = self.submodule_dir(path);
        std::fs::symlink_metadata(dir.join(".git")).ok()?;
        let opener = CliOpener {
            git: self.git.clone(),
        };
        let sub = opener.open_dir(&dir, &path.to_string_lossy()).ok()?;
        let same = match (
            std::fs::canonicalize(&sub.workdir),
            std::fs::canonicalize(&dir),
        ) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        same.then_some(sub)
    }

    /// The git directory a removed checkout left in the superproject (`git rev-parse --git-path
    /// modules/<name>`, after `git submodule deinit -f` or deleting the folder), as a repo whose
    /// commands run inside that directory. `git submodule update --init` would reuse it and move
    /// its HEAD. `None` if there is none (or the name could leave `modules/`).
    fn leftover_git_dir(&self, name: &str) -> Option<CliBackend> {
        if name.is_empty()
            || name.starts_with('/')
            || name.split(['/', '\\']).any(|part| part == "..")
        {
            return None;
        }
        let arg = format!("modules/{name}");
        let out = self
            .git
            .output(Some(&self.workdir), &["rev-parse", "--git-path", &arg])
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let dir = path_from_output(out.stdout.trim_ascii_end());
        let dir = if dir.is_absolute() {
            dir
        } else {
            self.workdir.join(dir)
        };
        if !dir.join("HEAD").is_file() {
            return None;
        }
        Some(CliBackend {
            git: self.git.clone(),
            workdir: dir.clone(),
            git_dir: dir,
            empty_tree: self.empty_tree,
            empty_blob: self.empty_blob,
        })
    }

    /// The state of a checked-out submodule, read inside it: `(head, detached, changes,
    /// untracked)`.
    fn submodule_worktree(
        sub: &CliBackend,
    ) -> Result<(Option<Oid>, bool, bool, bool), EngineError> {
        let args = [
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ];
        let status = parse_status(&sub.run(&args)?)?;
        let changes = status
            .entries
            .iter()
            .any(|e| !matches!(e.kind, EntryKind::Untracked | EntryKind::Ignored));
        let untracked = status
            .entries
            .iter()
            .any(|e| matches!(e.kind, EntryKind::Untracked));
        let detached = matches!(status.head, Head::Detached(_));
        Ok((status.head.target(), detached, changes, untracked))
    }

    /// True if the submodule's HEAD has commits on no branch, tag or remote branch.
    fn unreferenced_head(sub: &CliBackend) -> Result<bool, EngineError> {
        let out = sub.run(&[
            "rev-list",
            "--max-count=1",
            "HEAD",
            "--not",
            "--branches",
            "--tags",
            "--remotes",
            "--",
        ])?;
        Ok(!out.trim_ascii().is_empty())
    }

    /// True if checking out `recorded` (already in the submodule) over `head` would overwrite an
    /// ignored file: one at a path `recorded` adds, or at a parent folder of such a path. The
    /// paths in the way are found on disk, then `git check-ignore` says which are ignored
    /// (untracked ones are refused on their own, tracked ones are git's to replace). `false`
    /// when `recorded` is not in the submodule yet: it cannot be known before the fetch.
    fn ignored_in_the_way(sub: &CliBackend, head: Oid, recorded: Oid) -> Result<bool, EngineError> {
        if sub.resolve(&recorded.to_string())?.is_none() {
            return Ok(false);
        }
        let (head, recorded) = (head.to_string(), recorded.to_string());
        let added = sub.run(&[
            "diff-tree",
            "-r",
            "--name-only",
            "-z",
            "--no-renames",
            "--diff-filter=A",
            &head,
            &recorded,
            "--",
        ])?;
        let mut in_the_way: Vec<&[u8]> = Vec::new();
        for path in added.split(|&b| b == 0).filter(|p| !p.is_empty()) {
            let mut end = 0;
            loop {
                end = match path[end..].iter().position(|&b| b == b'/') {
                    Some(slash) => end + slash,
                    None => path.len(),
                };
                let prefix = &path[..end];
                let Ok(meta) = std::fs::symlink_metadata(sub.workdir.join(os_from_bytes(prefix)))
                else {
                    break;
                };
                if end == path.len() || !meta.is_dir() {
                    in_the_way.push(prefix);
                    break;
                }
                end += 1;
            }
        }
        if in_the_way.is_empty() {
            return Ok(false);
        }
        in_the_way.sort();
        in_the_way.dedup();
        let input = in_the_way.join(&0u8);
        let args = ["check-ignore", "-z", "--stdin"];
        let out = sub
            .git
            .output_with_input(Some(&sub.workdir), &args, &input)?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(failure(&args, &out)),
        }
    }

    /// Reads one submodule: inside its checkout, or in its leftover git directory when it is
    /// not checked out.
    fn inspect_submodule(
        &self,
        path: RepoPath,
        name: Option<String>,
        url: Option<String>,
        recorded: Option<Oid>,
    ) -> Result<SubmoduleInfo, EngineError> {
        let mut info = SubmoduleInfo {
            path,
            name,
            url,
            recorded,
            head: None,
            detached: false,
            state: SubmoduleState::Uninitialized,
            changes: false,
            untracked: false,
            operation: None,
            unreferenced: false,
            ignored_overwritten: false,
        };
        let checked_out = match self.submodule_repo(&info.path) {
            Some(sub) => {
                let (head, detached, changes, untracked) = Self::submodule_worktree(&sub)?;
                info.head = head;
                info.detached = detached;
                info.changes = changes;
                info.untracked = untracked;
                info.operation = sub.operation();
                info.unreferenced = detached && Self::unreferenced_head(&sub)?;
                if let (Some(head), Some(recorded)) = (head, recorded)
                    && head != recorded
                {
                    info.ignored_overwritten = Self::ignored_in_the_way(&sub, head, recorded)?;
                }
                true
            }
            None => {
                if let Some(leftover) = info.name.as_deref().and_then(|n| self.leftover_git_dir(n))
                {
                    info.head = leftover.resolve("HEAD")?;
                    let on_branch = leftover
                        .git
                        .output(Some(&leftover.workdir), &["symbolic-ref", "-q", "HEAD"])?
                        .status
                        .success();
                    info.detached = !on_branch && info.head.is_some();
                    info.operation = leftover.operation();
                    info.unreferenced = info.detached && Self::unreferenced_head(&leftover)?;
                }
                false
            }
        };
        info.state = submodule_state(
            recorded.is_none(),
            checked_out,
            info.changes || info.untracked,
            info.head,
            recorded,
        );
        Ok(info)
    }

    /// The repo where a submodule's HEAD lives: its checkout, or its leftover git directory.
    fn submodule_head_repo(&self, info: &SubmoduleInfo) -> Option<CliBackend> {
        self.submodule_repo(&info.path).or_else(|| {
            info.name
                .as_deref()
                .and_then(|name| self.leftover_git_dir(name))
        })
    }

    /// [`GitSubmodules::submodules`](project_engine::GitSubmodules::submodules).
    pub(super) fn list_submodules(&self) -> Result<Vec<SubmoduleInfo>, EngineError> {
        let links = self.gitlinks()?;
        if links.is_empty() {
            return Ok(Vec::new());
        }
        // `.gitmodules` of the worktree: path → name, name → URL.
        let mut names = BTreeMap::new();
        let mut module_urls = BTreeMap::new();
        let modules = self.config_regexp(Some(".gitmodules"), r"^submodule\..*\.(path|url)$")?;
        for (name, variable, value) in submodule_keys(&modules) {
            match variable.as_str() {
                "path" => {
                    names.insert(RepoPath::from_bytes(value), name);
                }
                "url" => {
                    module_urls.insert(name, String::from_utf8_lossy(&value).into_owned());
                }
                _ => {}
            }
        }
        let mut urls = BTreeMap::new();
        for (name, _, value) in submodule_keys(&self.config_regexp(None, r"^submodule\..*\.url$")?)
        {
            urls.insert(name, String::from_utf8_lossy(&value).into_owned());
        }

        let mut list = Vec::new();
        for (path, recorded) in links {
            let name = names.get(&path).cloned();
            // Credentials in URLs are never shown (as `remote.list`).
            let url = name
                .as_ref()
                .and_then(|n| urls.get(n).or_else(|| module_urls.get(n)))
                .map(|u| mask_url(u).0);
            list.push(self.inspect_submodule(path, name, url, recorded)?);
        }
        Ok(list)
    }

    /// [`GitSubmodules::update_submodules`](project_engine::GitSubmodules::update_submodules).
    pub(super) fn update_submodule_paths(
        &self,
        request: &SubmoduleUpdate,
        job: &JobContext,
    ) -> Result<SubmoduleUpdated, EngineError> {
        const ID: &str = "submodule.update";
        let plan = plan_update(self.list_submodules()?, request)?;
        let mut result = SubmoduleUpdated {
            unchanged: plan.unchanged.iter().map(|s| s.path.clone()).collect(),
            refused: plan.refused,
            ..SubmoduleUpdated::default()
        };

        // Each submodule is read again just before its backup: one that changed since the
        // checks is refused. The detached HEADs about to move are saved inside each submodule
        // (or its leftover git directory); a HEAD on a branch stays on that branch.
        let mut backups: BTreeMap<RepoPath, String> = BTreeMap::new();
        let mut run = Vec::new();
        for info in plan.run {
            let fresh = self.inspect_submodule(
                info.path.clone(),
                info.name.clone(),
                info.url.clone(),
                info.recorded,
            )?;
            let reason = if fresh.head != info.head
                || fresh.detached != info.detached
                || fresh.state != info.state
            {
                Some(SubmoduleRefusalReason::ChangedMeanwhile)
            } else {
                update_refusal(&fresh, request.init)
            };
            if let Some(reason) = reason {
                result.refused.push(SubmoduleRefusal {
                    message: refusal_message(&info.path, reason),
                    path: info.path,
                    reason,
                });
                continue;
            }
            if let (true, Some(head)) = (info.detached, info.head) {
                let repo = self.submodule_head_repo(&info).ok_or_else(|| {
                    EngineError::NotAllowed(refusal_message(
                        &info.path,
                        SubmoduleRefusalReason::ChangedMeanwhile,
                    ))
                })?;
                let name = repo.create_backup_ref(
                    SUBMODULE_BACKUP_KIND,
                    &head.to_string(),
                    "project: backup of the submodule HEAD before submodule.update",
                )?;
                backups.insert(info.path.clone(), name);
            }
            run.push(info);
        }
        result.refused.sort_by(|a, b| a.path.cmp(&b.path));

        if !run.is_empty() {
            if job.is_cancelled() {
                return Err(EngineError::Cancelled { command: ID });
            }
            result.messages = self.run_submodule_update(&run, request.init, job, &backups)?;
        }
        for info in run {
            let to = self
                .submodule_repo(&info.path)
                .map(|sub| sub.resolve("HEAD"))
                .transpose()?
                .flatten();
            if to != info.recorded {
                result
                    .warnings
                    .push(not_at_recorded(&info.path, info.recorded, to));
            }
            result.updated.push(SubmoduleMoved {
                backup: backups.remove(&info.path),
                path: info.path,
                from: info.head,
                to,
            });
        }

        if request.recursive {
            let mut paths: Vec<RepoPath> = result.updated.iter().map(|m| m.path.clone()).collect();
            paths.extend(result.unchanged.iter().cloned());
            paths.sort();
            for path in paths {
                if job.is_cancelled() {
                    return Err(EngineError::Cancelled { command: ID });
                }
                let Some(sub) = self.submodule_repo(&path) else {
                    continue;
                };
                let nested = sub.update_submodule_paths(
                    &SubmoduleUpdate {
                        paths: None,
                        init: request.init,
                        recursive: true,
                    },
                    job,
                )?;
                result.absorb_nested(&path, nested);
            }
        }
        Ok(result)
    }

    /// Runs `git submodule update` for the submodules of `run` (never empty); returns git's
    /// non-progress lines. On failure or cancellation after some backups were saved, the
    /// error names them.
    fn run_submodule_update(
        &self,
        run: &[SubmoduleInfo],
        init: bool,
        job: &JobContext,
        backups: &BTreeMap<RepoPath, String>,
    ) -> Result<Vec<String>, EngineError> {
        let mut args = vec![
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "protocol.fd.allow=never",
        ];
        args.extend(file_protocol_args(&self.git, &self.workdir));
        args.extend(["--literal-pathspecs", "submodule", "update"]);
        if init {
            args.push("--init");
        }
        args.extend(["--no-recommend-shallow", "--checkout", "--progress", "--"]);
        let paths: Vec<_> = run
            .iter()
            .map(|s| os_from_bytes(s.path.as_bytes()))
            .collect();
        let paths: Vec<&OsStr> = paths.iter().map(|p| p.as_os_str()).collect();
        // The remote named in credential questions and errors.
        let remote = match run {
            [one] => one.url.clone().unwrap_or_else(|| one.path.to_string()),
            _ => "submodules".to_owned(),
        };

        let mut lines = Vec::new();
        let streamed = self.git.run_network_with_paths(
            &self.workdir,
            &args,
            &paths,
            false,
            &remote,
            |Segment { text, newline }| match parse_progress(&text) {
                Some(progress) => job.report(&progress),
                None if newline && !text.trim().is_empty() => {
                    lines.push(text.trim_end().to_owned())
                }
                None => {}
            },
            &|| job.is_cancelled(),
        )?;
        let error = if streamed.cancelled {
            EngineError::Cancelled {
                command: "submodule.update",
            }
        } else if !streamed.status.success() {
            let stderr = lines.join("\n");
            streamed.remote_error(&remote, &stderr).unwrap_or_else(|| {
                failure(
                    &args,
                    &Output {
                        status: streamed.status,
                        stdout: Vec::new(),
                        stderr: stderr.into_bytes(),
                    },
                )
            })
        } else {
            return Ok(lines);
        };
        if backups.is_empty() {
            return Err(error);
        }
        let names: Vec<String> = backups
            .iter()
            .map(|(path, name)| format!("{path}: {name}"))
            .collect();
        Err(EngineError::GitWithReason {
            command: "submodule".to_owned(),
            reason: msg!(
                "engine-submodule-update-failed",
                error = error.to_string(),
                backups = names.join(", ")
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::submodule_keys;

    #[test]
    fn reads_submodule_keys() {
        let out = b"submodule.a.b.path\nlib/a b\0submodule.x.url\n../x\0core.bare\nfalse\0";
        let keys = submodule_keys(out);
        assert_eq!(
            keys,
            [
                ("a.b".to_owned(), "path".to_owned(), b"lib/a b".to_vec()),
                ("x".to_owned(), "url".to_owned(), b"../x".to_vec()),
            ]
        );
    }
}
