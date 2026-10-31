//! Git LFS in the view (S63, `impl-docs/git/reading.md` "Git LFS"): which files are stored in LFS
//! (the "LFS" badge in the stage area and the diff, "Stored in Git LFS (`<size>`)" in place of the
//! pointer text, no line operations) and the notice when the repo uses LFS but git-lfs is not
//! installed. Reads only: `repo.lfs_status` once per opening, `repo.lfs_paths` once per status
//! with the paths listed (only if the repo uses LFS) and for a diff whose path is not known yet.

use crate::{Request, Tab, Ticket, decode, log_failure};
use project_domain::{FileDiff, LineKind, RepoPath};
use project_engine::{LfsPathsParams, LfsPathsResult, LfsState};
use project_i18n::tr;
use serde_json::json;
use std::collections::{HashMap, HashSet};

pub(crate) const STATUS: &str = "repo.lfs_status";
pub(crate) const PATHS: &str = "repo.lfs_paths";

/// The actions of this area, dispatched by [`Tab::apply`](crate::Tab::apply).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Closes the notice "git-lfs is not installed".
    Dismiss,
}

/// What the view knows about LFS in the open repo.
#[derive(Debug, Default)]
pub struct LfsBox {
    /// The answer of `repo.lfs_status`; `None` until it arrives.
    state: Option<LfsState>,
    /// The status (`RepoView::status_seq`) whose paths were last sent to `repo.lfs_paths`.
    asked_status: Option<u64>,
    /// The paths asked to `repo.lfs_paths` and not answered yet.
    asking: HashSet<RepoPath>,
    /// The paths sent with each `repo.lfs_paths` on its way.
    sent: HashMap<Ticket, Vec<RepoPath>>,
    /// The answers so far: `true` for a file stored in LFS. Each status asks its paths again (a
    /// `.gitattributes` may have changed); the answers for diffs of commits stay.
    known: HashMap<RepoPath, bool>,
    /// The notice "git-lfs is not installed" is shown.
    notice: bool,
}

impl LfsBox {
    /// True once `repo.lfs_status` said the repo has LFS files.
    pub fn used(&self) -> bool {
        self.state.is_some_and(|s| s.used)
    }

    /// True if `path` is known to be stored in Git LFS.
    pub fn is_lfs(&self, path: &RepoPath) -> bool {
        self.known.get(path).copied().unwrap_or(false)
    }

    /// Whether `path` is stored in LFS: `None` while it is not known.
    pub fn lookup(&self, path: &RepoPath) -> Option<bool> {
        if !self.used() {
            return self.state.map(|_| false);
        }
        self.known.get(path).copied()
    }

    /// The notice "git-lfs is not installed" is shown.
    pub fn notice(&self) -> bool {
        self.notice
    }

    pub fn dismiss(&mut self) {
        self.notice = false;
    }

    /// The answer of `repo.lfs_status`. `first`: the notice was not shown yet for this repo in
    /// this session; returns true if it is shown now.
    pub fn receive_state(&mut self, state: LfsState, first: bool) -> bool {
        self.state = Some(state);
        self.notice = self.notice || (first && state.missing_git_lfs());
        self.notice
    }

    /// The paths of status `seq` to send to `repo.lfs_paths`, once per status and only if the
    /// repo uses LFS (empty: nothing to send).
    fn status_paths(&mut self, seq: u64, paths: Vec<RepoPath>) -> Vec<RepoPath> {
        if !self.used() || self.asked_status == Some(seq) {
            return Vec::new();
        }
        self.asked_status = Some(seq);
        self.asking.extend(paths.iter().cloned());
        paths
    }

    /// The path of a loaded diff to send to `repo.lfs_paths`, if it is not known nor asked.
    fn diff_path(&mut self, path: &RepoPath) -> Option<RepoPath> {
        if !self.used() || self.known.contains_key(path) || self.asking.contains(path) {
            return None;
        }
        self.asking.insert(path.clone());
        Some(path.clone())
    }

    /// The answer of `repo.lfs_paths` sent with `ticket`: `lfs` are those stored in LFS. After
    /// an error (`None`) the paths count as not in LFS, so they are not asked again in a loop.
    fn receive_paths(&mut self, ticket: Ticket, lfs: Option<&[RepoPath]>) {
        let lfs: HashSet<&RepoPath> = lfs.unwrap_or_default().iter().collect();
        for path in self.sent.remove(&ticket).unwrap_or_default() {
            self.asking.remove(&path);
            let stored = lfs.contains(&path);
            self.known.insert(path, stored);
        }
    }
}

/// "Stored in Git LFS (`<size>`)": the size from the pointer's `size` line of the new side (of
/// the old one for a deleted file); without it, the text without size.
pub fn note(file: &FileDiff) -> String {
    let size = |skip: LineKind| {
        file.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind != skip)
            .find_map(|l| {
                let text = l.content.to_str()?;
                text.trim_end().strip_prefix("size ")?.parse::<u64>().ok()
            })
    };
    match size(LineKind::Removed).or_else(|| size(LineKind::Added)) {
        Some(bytes) => tr!("lfs-stored", size = size_text(bytes)),
        None => tr!("lfs-stored-unknown"),
    }
}

/// A size in bytes for people: "512 bytes", "1.5 KB", "12.0 MB", "1.2 GB".
pub fn size_text(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        tr!("lfs-size-bytes", n = bytes)
    } else if b < K * K {
        tr!("lfs-size-kb", n = format!("{:.1}", b / K))
    } else if b < K * K * K {
        tr!("lfs-size-mb", n = format!("{:.1}", b / (K * K)))
    } else {
        tr!("lfs-size-gb", n = format!("{:.1}", b / (K * K * K)))
    }
}

impl Tab {
    pub(crate) fn apply_lfs(&mut self, action: Action) {
        match action {
            Action::Dismiss => {
                if let Some(repo) = &mut self.repo {
                    repo.lfs.dismiss();
                }
            }
        }
    }

    /// Asks `repo.lfs_status` (on opening and on "Refresh").
    pub(crate) fn request_lfs_status(&mut self, generation: u64) {
        self.submit(STATUS, json!({}), Request::LfsStatus(generation));
    }

    /// After every answer: the paths of a new status and the path of a loaded diff not known
    /// yet go to `repo.lfs_paths`; the open diff shows the LFS note if its file is in LFS.
    pub(crate) fn follow_lfs(&mut self) {
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        if !repo.lfs.used() {
            return;
        }
        if let Some(status) = &repo.status {
            let paths = status.entries.iter().map(|e| e.path.clone()).collect();
            let paths = repo.lfs.status_paths(repo.status_seq, paths);
            if !paths.is_empty() {
                self.send_paths(generation, paths);
            }
        }
        let Some(repo) = &mut self.repo else {
            return;
        };
        let Some((target, _, _)) = repo.diff.loaded() else {
            return;
        };
        let path = target.path.clone();
        match repo.lfs.lookup(&path) {
            Some(lfs) => repo.diff.set_lfs(lfs),
            None => {
                if let Some(path) = repo.lfs.diff_path(&path) {
                    self.send_paths(generation, vec![path]);
                }
            }
        }
    }

    fn send_paths(&mut self, generation: u64, paths: Vec<RepoPath>) {
        let params = crate::params(LfsPathsParams {
            paths: paths.clone(),
        });
        let ticket = self.submit(PATHS, params, Request::LfsPaths(generation));
        if let Some(repo) = &mut self.repo {
            repo.lfs.sent.insert(ticket, paths);
        }
    }

    /// The answers of `repo.lfs_status` and `repo.lfs_paths`: on error the files are shown as
    /// any other (the error goes to the log only).
    pub(crate) fn receive_lfs(&mut self, request: Request, done: crate::Done) {
        log_failure(&done);
        match request {
            Request::LfsStatus(generation) => {
                let Ok(state) = done
                    .result
                    .and_then(|v| decode::<LfsState>(&done.command, v))
                else {
                    return;
                };
                let noticed = &mut self.lfs_noticed;
                let Some(repo) = self.repo.as_mut().filter(|_| generation == self.generation)
                else {
                    return;
                };
                let first = !noticed.contains(&repo.path);
                if repo.lfs.receive_state(state, first) {
                    noticed.insert(repo.path.clone());
                }
            }
            Request::LfsPaths(generation) => {
                let lfs = done
                    .result
                    .and_then(|v| decode::<LfsPathsResult>(&done.command, v))
                    .ok();
                if let Some(repo) = self.current(generation) {
                    let lfs = lfs.as_ref().map(|r| r.paths.as_slice());
                    repo.lfs.receive_paths(done.ticket, lfs);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "lfs_tests.rs"]
mod tests;
