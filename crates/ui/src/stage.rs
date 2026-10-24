//! Stage area: the entries of `repo.status` split into "staged" and "unstaged", the paths
//! to send to `index.stage`/`index.unstage` and what to do with the open diff after a write.
//! Data only, no egui: tested with unit tests.

use crate::{Action, Request, Tab, stage};
use project_domain::{Change, Conflict, EntryKind, RepoPath, Status, StatusEntry};
use project_i18n::tr;
use serde_json::json;

/// One of the two lists of the stage area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// Staged changes (HEAD -> index).
    Staged,
    /// Unstaged changes (index -> worktree), untracked and conflicted files.
    Unstaged,
}

impl Side {
    pub fn other(self) -> Self {
        match self {
            Side::Staged => Side::Unstaged,
            Side::Unstaged => Side::Staged,
        }
    }
}

/// A file in one of the two lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageItem {
    pub path: RepoPath,
    /// The change of this list: the letter `X` for "staged", `Y` for "unstaged";
    /// [`Change::Untracked`] and [`Change::Unmerged`] for untracked and conflicted files.
    pub change: Change,
    /// Source path of a staged rename or copy.
    pub original: Option<RepoPath>,
    /// Kind of conflict, for a conflicted file.
    pub conflict: Option<Conflict>,
}

impl StageItem {
    pub fn is_conflict(&self) -> bool {
        self.conflict.is_some()
    }
}

/// The two lists of the stage area, rebuilt when a new [`Status`] arrives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageLists {
    pub staged: Vec<StageItem>,
    /// Conflicts first, then modified tracked files, then untracked ones; within each
    /// group git's order.
    pub unstaged: Vec<StageItem>,
}

impl StageLists {
    /// A file with changes both staged and unstaged appears in both lists. Ignored
    /// files do not appear.
    pub fn new(status: &Status) -> Self {
        let mut lists = Self::default();
        for entry in &status.entries {
            if let EntryKind::Unmerged(info) = &entry.kind {
                lists
                    .unstaged
                    .push(item(entry, Change::Unmerged, Some(info.conflict)));
                continue;
            }
            if entry.is_staged() {
                let mut staged = item(entry, entry.index, None);
                staged.original = entry.original_path().cloned();
                lists.staged.push(staged);
            }
            if entry.is_untracked() {
                lists.unstaged.push(item(entry, Change::Untracked, None));
            } else if entry.is_unstaged() {
                lists.unstaged.push(item(entry, entry.worktree, None));
            }
        }
        lists.unstaged.sort_by_key(|item| match item.change {
            Change::Unmerged => 0,
            Change::Untracked => 2,
            _ => 1,
        });
        lists
    }

    pub fn side(&self, side: Side) -> &[StageItem] {
        match side {
            Side::Staged => &self.staged,
            Side::Unstaged => &self.unstaged,
        }
    }

    pub fn contains(&self, side: Side, path: &RepoPath) -> bool {
        self.side(side).iter().any(|item| item.path == *path)
    }

    pub fn is_empty(&self) -> bool {
        self.staged.is_empty() && self.unstaged.is_empty()
    }

    /// The conflicted files among `paths`: staging them marks them resolved.
    pub fn conflicts_among(&self, paths: &[RepoPath]) -> Vec<RepoPath> {
        self.unstaged
            .iter()
            .filter(|item| item.is_conflict() && paths.contains(&item.path))
            .map(|item| item.path.clone())
            .collect()
    }
}

fn item(entry: &StatusEntry, change: Change, conflict: Option<Conflict>) -> StageItem {
    StageItem {
        path: entry.path.clone(),
        change,
        original: None,
        conflict,
    }
}

/// How many entries count as uncommitted changes (all except ignored).
pub fn change_count(status: &Status) -> usize {
    status
        .entries
        .iter()
        .filter(|entry| !matches!(entry.kind, EntryKind::Ignored))
        .count()
}

/// The paths of a whole list, for "Stage all" and "Unstage all": always explicit,
/// never an empty list (for git "no paths" would mean "the whole repo").
pub fn all_paths(items: &[StageItem]) -> Option<Vec<RepoPath>> {
    let mut paths: Vec<RepoPath> = Vec::with_capacity(items.len());
    for item in items {
        if !paths.contains(&item.path) {
            paths.push(item.path.clone());
        }
    }
    (!paths.is_empty()).then_some(paths)
}

/// A write on the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageOp {
    Stage,
    Unstage,
}

impl StageOp {
    /// The id of the Registry command.
    pub fn command(self) -> &'static str {
        match self {
            StageOp::Stage => "index.stage",
            StageOp::Unstage => "index.unstage",
        }
    }

    /// Name of the operation for messages, e.g. "Staging 2 files".
    pub fn describe(self, paths: &[RepoPath]) -> String {
        match (self, paths) {
            (StageOp::Stage, [one]) => {
                tr!(
                    "stage-label-stage-one",
                    path = one.to_string_lossy().into_owned()
                )
            }
            (StageOp::Stage, many) => tr!("stage-label-stage-many", n = many.len()),
            (StageOp::Unstage, [one]) => {
                tr!(
                    "stage-label-unstage-one",
                    path = one.to_string_lossy().into_owned()
                )
            }
            (StageOp::Unstage, many) => tr!("stage-label-unstage-many", n = many.len()),
        }
    }
}

/// A write to confirm before sending it: staging conflicted files marks them
/// resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWrite {
    pub op: StageOp,
    pub paths: Vec<RepoPath>,
    /// The conflicted files among `paths`.
    pub conflicts: Vec<RepoPath>,
}

/// What to ask for a write: send it at once, or confirm first. `None` if there is
/// nothing to write.
pub fn plan_write(lists: &StageLists, op: StageOp, paths: Vec<RepoPath>) -> Option<PendingWrite> {
    if paths.is_empty() {
        return None;
    }
    let conflicts = match op {
        StageOp::Stage => lists.conflicts_among(&paths),
        StageOp::Unstage => Vec::new(),
    };
    Some(PendingWrite {
        op,
        paths,
        conflicts,
    })
}

impl PendingWrite {
    /// True if it must be confirmed before sending.
    pub fn needs_confirm(&self) -> bool {
        !self.conflicts.is_empty()
    }
}

/// What to do with the open diff of a file of the list `side` after a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    /// The file is still in the same list: the diff is requested again.
    Reload,
    /// The file moved entirely to the other list: the diff of that one opens.
    Switch(Side),
    /// The file has no changes anymore: the diff closes.
    Close,
}

pub fn follow(lists: &StageLists, side: Side, path: &RepoPath) -> Follow {
    if lists.contains(side, path) {
        Follow::Reload
    } else if lists.contains(side.other(), path) {
        Follow::Switch(side.other())
    } else {
        Follow::Close
    }
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_stage(&mut self, action: Action) {
        match action {
            Action::Write(op, paths) => {
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                if let Some(write) = stage::plan_write(&repo.stage, op, paths) {
                    if write.needs_confirm() {
                        repo.confirm = Some(write);
                    } else {
                        self.send_write(write);
                    }
                }
            }
            Action::ConfirmWrite => {
                if let Some(write) = self.repo.as_mut().and_then(|r| r.confirm.take()) {
                    self.send_write(write);
                }
            }
            Action::CancelWrite => {
                if let Some(repo) = &mut self.repo {
                    repo.confirm = None;
                }
            }
            Action::DismissWriteError => {
                if let Some(repo) = &mut self.repo {
                    repo.write_error = None;
                }
            }
            _ => unreachable!("action dispatched to the wrong function"),
        }
    }

    /// Sends a write on the index, if there is not one in progress already: never two together.
    pub(crate) fn send_write(&mut self, write: PendingWrite) {
        if self.executor.writing().is_some() || write.paths.is_empty() {
            return;
        }
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        repo.write_error = None;
        repo.write_label = Some(write.op.describe(&write.paths));
        let params = json!({ "paths": write.paths });
        self.submit(write.op.command(), params, Request::Write(generation));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Head, Oid, TrackedInfo, UnmergedInfo};

    const ZERO: Oid = Oid::from_bytes([0; 20]);

    fn tracked(path: &str, index: Change, worktree: Change) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            index,
            worktree,
            kind: EntryKind::Ordinary(TrackedInfo {
                submodule: None,
                mode_head: 0o100644,
                mode_index: 0o100644,
                mode_worktree: 0o100644,
                oid_head: ZERO,
                oid_index: ZERO,
            }),
        }
    }

    fn renamed(path: &str, original: &str, worktree: Change) -> StatusEntry {
        let EntryKind::Ordinary(info) = tracked(path, Change::Renamed, worktree).kind else {
            unreachable!()
        };
        StatusEntry {
            path: path.into(),
            index: Change::Renamed,
            worktree,
            kind: EntryKind::Renamed {
                info,
                original: original.into(),
                score: 90,
            },
        }
    }

    fn simple(path: &str, change: Change, kind: EntryKind) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            index: change,
            worktree: change,
            kind,
        }
    }

    fn conflict(path: &str) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            index: Change::Unmerged,
            worktree: Change::Unmerged,
            kind: EntryKind::Unmerged(UnmergedInfo {
                conflict: Conflict::BothModified,
                submodule: None,
                mode_base: 0o100644,
                mode_ours: 0o100644,
                mode_theirs: 0o100644,
                mode_worktree: 0o100644,
                oid_base: ZERO,
                oid_ours: ZERO,
                oid_theirs: ZERO,
            }),
        }
    }

    fn status(entries: Vec<StatusEntry>) -> Status {
        Status {
            head: Head::Branch {
                name: "main".to_owned(),
                target: None,
            },
            upstream: None,
            entries,
            operation: None,
        }
    }

    fn sample() -> Status {
        status(vec![
            tracked("README.md", Change::Modified, Change::Modified),
            tracked("docs/guide.md", Change::Added, Change::Unmodified),
            renamed("new.rs", "old.rs", Change::Unmodified),
            tracked("src/lib.rs", Change::Unmodified, Change::Modified),
            tracked("unused.txt", Change::Unmodified, Change::Deleted),
            conflict("zeta.rs"),
            simple("note.txt", Change::Untracked, EntryKind::Untracked),
            simple("build/out", Change::Ignored, EntryKind::Ignored),
        ])
    }

    fn paths(items: &[StageItem]) -> Vec<String> {
        items.iter().map(|i| i.path.to_string()).collect()
    }

    #[test]
    fn entries_are_grouped_in_staged_and_unstaged() {
        let lists = StageLists::new(&sample());
        assert_eq!(
            paths(&lists.staged),
            ["README.md", "docs/guide.md", "new.rs"]
        );
        // Conflicts on top, untracked at the bottom, ignored excluded.
        assert_eq!(
            paths(&lists.unstaged),
            [
                "zeta.rs",
                "README.md",
                "src/lib.rs",
                "unused.txt",
                "note.txt"
            ]
        );
        let changes: Vec<Change> = lists.staged.iter().map(|i| i.change).collect();
        assert_eq!(changes, [Change::Modified, Change::Added, Change::Renamed]);
        assert_eq!(lists.staged[2].original, Some("old.rs".into()));
        let changes: Vec<Change> = lists.unstaged.iter().map(|i| i.change).collect();
        assert_eq!(
            changes,
            [
                Change::Unmerged,
                Change::Modified,
                Change::Modified,
                Change::Deleted,
                Change::Untracked
            ]
        );
        assert_eq!(lists.unstaged[0].conflict, Some(Conflict::BothModified));
        assert!(lists.unstaged[1..].iter().all(|i| !i.is_conflict()));
        assert!(lists.contains(Side::Staged, &"README.md".into()));
        assert!(lists.contains(Side::Unstaged, &"README.md".into()));
        assert!(!lists.contains(Side::Staged, &"note.txt".into()));
        assert_eq!(change_count(&sample()), 7);
    }

    #[test]
    fn an_empty_status_has_empty_lists() {
        let empty = status(vec![simple("x", Change::Ignored, EntryKind::Ignored)]);
        assert!(StageLists::new(&empty).is_empty());
        assert_eq!(change_count(&empty), 0);
    }

    #[test]
    fn all_paths_is_explicit_and_never_empty() {
        let lists = StageLists::new(&sample());
        assert_eq!(all_paths(&[]), None);
        let staged = all_paths(&lists.staged).unwrap();
        assert_eq!(
            staged,
            ["README.md", "docs/guide.md", "new.rs"].map(RepoPath::from)
        );
        let unstaged = all_paths(&lists.unstaged).unwrap();
        assert_eq!(unstaged.len(), 5);
        // Without duplicates even if an entry were repeated.
        let twice = [lists.staged[0].clone(), lists.staged[0].clone()];
        assert_eq!(all_paths(&twice).unwrap().len(), 1);
    }

    #[test]
    fn staging_a_conflict_needs_a_confirmation() {
        let lists = StageLists::new(&sample());
        let all = all_paths(&lists.unstaged).unwrap();
        let write = plan_write(&lists, StageOp::Stage, all.clone()).unwrap();
        assert!(write.needs_confirm());
        assert_eq!(write.conflicts, [RepoPath::from("zeta.rs")]);

        let plain = plan_write(&lists, StageOp::Stage, vec!["note.txt".into()]).unwrap();
        assert!(!plain.needs_confirm());
        let unstage = plan_write(&lists, StageOp::Unstage, all).unwrap();
        assert!(!unstage.needs_confirm());
        assert_eq!(plan_write(&lists, StageOp::Stage, Vec::new()), None);
    }

    #[test]
    fn ops_name_their_command_and_list() {
        assert_eq!(StageOp::Stage.command(), "index.stage");
        assert_eq!(StageOp::Unstage.command(), "index.unstage");
        assert_eq!(
            StageOp::Stage.describe(&["note.txt".into()]),
            "Staging note.txt"
        );
        assert_eq!(
            StageOp::Unstage.describe(&["a".into(), "b".into()]),
            "Unstaging 2 files"
        );
    }

    #[test]
    fn the_open_diff_follows_the_file() {
        let lists = StageLists::new(&sample());
        let follow = |side, path: &str| follow(&lists, side, &path.into());
        assert_eq!(follow(Side::Unstaged, "README.md"), Follow::Reload);
        assert_eq!(follow(Side::Staged, "README.md"), Follow::Reload);
        assert_eq!(
            follow(Side::Unstaged, "docs/guide.md"),
            Follow::Switch(Side::Staged)
        );
        assert_eq!(
            follow(Side::Staged, "src/lib.rs"),
            Follow::Switch(Side::Unstaged)
        );
        assert_eq!(follow(Side::Staged, "gone.txt"), Follow::Close);
    }

    /// The full path with the sample repo of the `MemoryBackend`, as the UI does it:
    /// `repo.status`, then `index.stage` with the paths of a list, and the state from the response.
    #[test]
    fn staging_with_the_sample_repo_moves_the_file() {
        use project_engine::{MemoryOpener, Registry, Session};
        use serde_json::json;
        use std::sync::Arc;

        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(MemoryOpener::sample()));
        let path = MemoryOpener::SAMPLE_PATH;
        registry
            .run("repo.open", &mut session, json!({ "path": path }))
            .unwrap();
        let status: Status = serde_json::from_value(
            registry
                .run("repo.status", &mut session, json!({}))
                .unwrap(),
        )
        .unwrap();
        let lists = StageLists::new(&status);
        assert!(lists.contains(Side::Unstaged, &"note.txt".into()));
        assert!(!lists.contains(Side::Staged, &"note.txt".into()));

        let params = json!({ "paths": ["note.txt"] });
        let out = registry.run("index.stage", &mut session, params).unwrap();
        let lists = StageLists::new(&serde_json::from_value(out["status"].clone()).unwrap());
        assert!(lists.contains(Side::Staged, &"note.txt".into()));
        assert!(!lists.contains(Side::Unstaged, &"note.txt".into()));
        assert_eq!(
            follow(&lists, Side::Unstaged, &"note.txt".into()),
            Follow::Switch(Side::Staged)
        );

        let all = all_paths(&lists.staged).unwrap();
        let out = registry
            .run("index.unstage", &mut session, json!({ "paths": all }))
            .unwrap();
        // README.md was `MM`: the staged version is saved in a backup.
        assert!(out["backup"].is_string(), "{out}");
        let lists = StageLists::new(&serde_json::from_value(out["status"].clone()).unwrap());
        assert!(lists.staged.is_empty());
        assert!(lists.contains(Side::Unstaged, &"note.txt".into()));
    }
}
