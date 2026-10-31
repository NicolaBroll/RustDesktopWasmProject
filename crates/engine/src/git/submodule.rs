//! Submodules (`submodule.list`, `submodule.update`, `impl-docs/git/submodules.md`): what the
//! backends report about each submodule and the per-submodule refusals of an update, shared by
//! the `CliBackend` and the `MemoryBackend`.
//!
//! An update never runs for a submodule that would lose something: local changes or untracked
//! files (git would refuse midway or leave them mixed with another commit), an operation in
//! progress inside it, or a detached HEAD with commits no branch, tag or remote branch contains
//! (after the update they would be reachable only from the reflog).

use super::backup::path_matches;
use crate::{EngineError, Message};
use project_domain::{Oid, Operation, RepoPath};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Prefix of the backup refs saved **inside a submodule** before an update moves its detached
/// HEAD: `refs/project/backup/submodule-<seconds>-<nanoseconds>`.
pub const SUBMODULE_BACKUP_KIND: &str = "submodule";

/// The state of a submodule (`submodule.list`), the most important one first when several apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmoduleState {
    /// Conflicted in the superproject's index (different commits recorded by the two sides).
    Conflict,
    /// Not checked out: no repository at its path (`git submodule status` `-`).
    Uninitialized,
    /// Checked out, with changes to tracked files or untracked files.
    LocalChanges,
    /// Checked out on a commit other than the one recorded in the superproject's index.
    DifferentCommit,
    /// Checked out on the recorded commit, clean.
    InSync,
}

/// A submodule of the open repo (`submodule.list`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmoduleInfo {
    /// Path in the superproject (the gitlink in the index).
    pub path: RepoPath,
    /// Name in `.gitmodules` (`submodule.<name>.path`); `None` if `.gitmodules` does not map
    /// the path (git cannot update it).
    pub name: Option<String>,
    /// `submodule.<name>.url` of the repo configuration (set by init), otherwise the one in
    /// `.gitmodules` as written (relative URLs stay relative).
    pub url: Option<String>,
    /// The commit recorded in the superproject's index (the one an update checks out); `None`
    /// when conflicted.
    pub recorded: Option<Oid>,
    /// The commit checked out in the submodule; `None` if not checked out (or no commits).
    pub head: Option<Oid>,
    /// The submodule's HEAD is detached (the usual state after an update).
    pub detached: bool,
    pub state: SubmoduleState,
    /// Changes to tracked files (staged or not), nested submodules included.
    pub changes: bool,
    /// Untracked files (ignored ones do not count).
    pub untracked: bool,
    /// Operation in progress inside the submodule (merge, rebase, ...).
    pub operation: Option<Operation>,
    /// The detached HEAD has commits not reachable from the submodule's branches, tags or remote
    /// branches: an update would leave them behind.
    pub unreferenced: bool,
}

/// Parameters of `submodule.update`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SubmoduleUpdate {
    /// The submodules to update (a folder selects those inside it); `None` = all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<RepoPath>>,
    /// Also clone and check out the uninitialized ones (`--init`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub init: bool,
    /// Then the submodules inside the updated ones, with the same checks, level by level.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub recursive: bool,
}

/// Why a submodule was not updated: nothing was run for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmoduleRefusalReason {
    /// Uninitialized and `init` was not requested.
    NotInitialized,
    /// `.gitmodules` has no entry for the path.
    NoMapping,
    /// The URL starts with `-`: it would be read as an option (git refuses it too).
    SuspiciousUrl,
    /// Conflicted in the superproject: resolve it first.
    Conflict,
    /// An operation is in progress inside the submodule.
    OperationInProgress,
    /// Changes to tracked files.
    LocalChanges,
    /// Untracked files.
    UntrackedFiles,
    /// The detached HEAD has commits on no branch, tag or remote branch.
    UnreferencedCommits,
}

/// A submodule left as it was, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmoduleRefusal {
    pub path: RepoPath,
    pub reason: SubmoduleRefusalReason,
    /// The reason as text.
    pub message: Message,
}

/// A submodule the update ran for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmoduleMoved {
    pub path: RepoPath,
    /// HEAD before (`None`: it was not checked out).
    pub from: Option<Oid>,
    /// HEAD after, as re-read.
    pub to: Option<Oid>,
    /// The backup ref of `from`, inside the submodule, when HEAD was detached.
    pub backup: Option<String>,
}

/// Result of `submodule.update` ([`GitSubmodules::update_submodules`](crate::GitSubmodules)).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmoduleUpdated {
    pub updated: Vec<SubmoduleMoved>,
    /// Already on the recorded commit: nothing to do.
    pub unchanged: Vec<RepoPath>,
    pub refused: Vec<SubmoduleRefusal>,
    /// git's non-progress lines.
    pub messages: Vec<String>,
    /// A submodule that after the update is not on the recorded commit (e.g. a hook moved it).
    pub warnings: Vec<Message>,
}

/// What an update does with the selected submodules ([`plan_update`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdatePlan {
    /// To pass to git.
    pub run: Vec<SubmoduleInfo>,
    pub unchanged: Vec<SubmoduleInfo>,
    pub refused: Vec<SubmoduleRefusal>,
}

impl SubmoduleUpdate {
    /// [`EngineError::InvalidParams`] for an empty `paths` list or an empty path.
    pub fn check(&self) -> Result<(), EngineError> {
        if let Some(paths) = &self.paths
            && (paths.is_empty() || paths.iter().any(|p| p.as_bytes().is_empty()))
        {
            return Err(EngineError::InvalidParams {
                id: "submodule.update",
                reason: msg!("engine-submodule-paths-empty"),
            });
        }
        Ok(())
    }
}

/// The state of a submodule from its parts (`changes`/`untracked` only count when checked out).
pub fn submodule_state(
    conflicted: bool,
    checked_out: bool,
    dirty: bool,
    head: Option<Oid>,
    recorded: Option<Oid>,
) -> SubmoduleState {
    if conflicted {
        SubmoduleState::Conflict
    } else if !checked_out {
        SubmoduleState::Uninitialized
    } else if dirty {
        SubmoduleState::LocalChanges
    } else if head != recorded {
        SubmoduleState::DifferentCommit
    } else {
        SubmoduleState::InSync
    }
}

/// The reason a submodule is not updated, `None` if the update may run for it. A submodule
/// already on the recorded commit is not refused (nothing would run): see [`plan_update`].
pub fn update_refusal(info: &SubmoduleInfo, init: bool) -> Option<SubmoduleRefusalReason> {
    use SubmoduleRefusalReason as R;
    if info.state == SubmoduleState::Conflict || info.recorded.is_none() {
        return Some(R::Conflict);
    }
    if info.name.is_none() {
        return Some(R::NoMapping);
    }
    if info.url.as_deref().is_some_and(|u| u.starts_with('-')) {
        return Some(R::SuspiciousUrl);
    }
    if info.state == SubmoduleState::Uninitialized {
        return (!init).then_some(R::NotInitialized);
    }
    if info.operation.is_some() {
        Some(R::OperationInProgress)
    } else if info.changes {
        Some(R::LocalChanges)
    } else if info.untracked {
        Some(R::UntrackedFiles)
    } else if info.unreferenced {
        Some(R::UnreferencedCommits)
    } else {
        None
    }
}

/// The text of a refusal.
pub fn refusal_message(path: &RepoPath, reason: SubmoduleRefusalReason) -> Message {
    use SubmoduleRefusalReason as R;
    let path = path.to_string_lossy().into_owned();
    match reason {
        R::NotInitialized => msg!("engine-submodule-not-initialized", path = path),
        R::NoMapping => msg!("engine-submodule-no-mapping", path = path),
        R::SuspiciousUrl => msg!("engine-submodule-suspicious-url", path = path),
        R::Conflict => msg!("engine-submodule-conflict", path = path),
        R::OperationInProgress => msg!("engine-submodule-operation", path = path),
        R::LocalChanges => msg!("engine-submodule-local-changes", path = path),
        R::UntrackedFiles => msg!("engine-submodule-untracked", path = path),
        R::UnreferencedCommits => msg!("engine-submodule-unreferenced", path = path),
    }
}

/// Selects the submodules of `request.paths` (all if `None`) and sorts them into those to run,
/// those already on the recorded commit and the refused ones. A requested path that is not a
/// submodule nor a folder containing one → [`EngineError::NotAllowed`], nothing runs.
pub fn plan_update(
    submodules: Vec<SubmoduleInfo>,
    request: &SubmoduleUpdate,
) -> Result<UpdatePlan, EngineError> {
    if let Some(paths) = &request.paths
        && let Some(missing) = paths
            .iter()
            .find(|p| !submodules.iter().any(|s| path_matches(&s.path, p)))
    {
        return Err(EngineError::NotAllowed(msg!(
            "engine-submodule-unknown",
            path = missing.to_string_lossy().into_owned()
        )));
    }
    let mut plan = UpdatePlan::default();
    for info in submodules {
        let selected = request
            .paths
            .as_ref()
            .is_none_or(|paths| paths.iter().any(|p| path_matches(&info.path, p)));
        if !selected {
            continue;
        }
        // Already on the recorded commit: git would do nothing, whatever the worktree holds.
        if info.state != SubmoduleState::Uninitialized
            && info.state != SubmoduleState::Conflict
            && info.head.is_some()
            && info.head == info.recorded
        {
            plan.unchanged.push(info);
            continue;
        }
        match update_refusal(&info, request.init) {
            Some(reason) => plan.refused.push(SubmoduleRefusal {
                message: refusal_message(&info.path, reason),
                path: info.path,
                reason,
            }),
            None => plan.run.push(info),
        }
    }
    Ok(plan)
}

/// `path` inside the submodule at `prefix`, for the results of a recursive update.
pub fn nested_path(prefix: &RepoPath, path: &RepoPath) -> RepoPath {
    let mut bytes = prefix.as_bytes().to_vec();
    bytes.push(b'/');
    bytes.extend_from_slice(path.as_bytes());
    RepoPath::from_bytes(bytes)
}

impl SubmoduleUpdated {
    /// Adds the result of the update inside the submodule `prefix`, with its paths prefixed.
    pub fn absorb_nested(&mut self, prefix: &RepoPath, nested: SubmoduleUpdated) {
        for mut moved in nested.updated {
            moved.path = nested_path(prefix, &moved.path);
            self.updated.push(moved);
        }
        self.unchanged.extend(
            nested
                .unchanged
                .iter()
                .map(|path| nested_path(prefix, path)),
        );
        for refusal in nested.refused {
            let path = nested_path(prefix, &refusal.path);
            self.refused.push(SubmoduleRefusal {
                message: refusal_message(&path, refusal.reason),
                path,
                reason: refusal.reason,
            });
        }
        self.messages.extend(nested.messages);
        self.warnings.extend(nested.warnings);
    }
}

/// The warning for a submodule that after a successful update is not on the recorded commit.
pub fn not_at_recorded(path: &RepoPath, recorded: Option<Oid>, head: Option<Oid>) -> Message {
    let show = |o: Option<Oid>| o.map_or_else(|| "-".to_owned(), |o| o.to_string());
    msg!(
        "engine-submodule-not-at-recorded",
        path = path.to_string_lossy().into_owned(),
        recorded = show(recorded),
        head = show(head)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use SubmoduleRefusalReason as R;

    fn oid(n: u8) -> Oid {
        Oid::from_bytes([n; Oid::LEN])
    }

    fn info(path: &str, head: Option<u8>) -> SubmoduleInfo {
        let head = head.map(oid);
        SubmoduleInfo {
            path: RepoPath::from(path),
            name: Some(path.to_owned()),
            url: Some("../sub".to_owned()),
            recorded: Some(oid(1)),
            head,
            detached: true,
            state: submodule_state(false, head.is_some(), false, head, Some(oid(1))),
            changes: false,
            untracked: false,
            operation: None,
            unreferenced: false,
        }
    }

    #[test]
    fn states_in_order_of_importance() {
        let (a, b) = (Some(oid(1)), Some(oid(2)));
        assert_eq!(
            submodule_state(true, true, true, a, b),
            SubmoduleState::Conflict
        );
        assert_eq!(
            submodule_state(false, false, true, a, b),
            SubmoduleState::Uninitialized
        );
        assert_eq!(
            submodule_state(false, true, true, a, b),
            SubmoduleState::LocalChanges
        );
        assert_eq!(
            submodule_state(false, true, false, a, b),
            SubmoduleState::DifferentCommit
        );
        assert_eq!(
            submodule_state(false, true, false, a, a),
            SubmoduleState::InSync
        );
    }

    #[test]
    fn refusals_per_submodule() {
        let base = info("s", Some(2));
        assert_eq!(update_refusal(&base, false), None);
        let with = |f: &dyn Fn(&mut SubmoduleInfo)| {
            let mut i = base.clone();
            f(&mut i);
            update_refusal(&i, false)
        };
        assert_eq!(with(&|i| i.changes = true), Some(R::LocalChanges));
        assert_eq!(with(&|i| i.untracked = true), Some(R::UntrackedFiles));
        assert_eq!(
            with(&|i| i.operation = Some(Operation::Merge)),
            Some(R::OperationInProgress)
        );
        assert_eq!(
            with(&|i| i.unreferenced = true),
            Some(R::UnreferencedCommits)
        );
        assert_eq!(with(&|i| i.name = None), Some(R::NoMapping));
        assert_eq!(
            with(&|i| i.url = Some("--upload-pack=x".into())),
            Some(R::SuspiciousUrl)
        );
        assert_eq!(
            with(&|i| {
                i.state = SubmoduleState::Conflict;
                i.recorded = None;
            }),
            Some(R::Conflict)
        );
        let uninit = info("u", None);
        assert_eq!(update_refusal(&uninit, false), Some(R::NotInitialized));
        assert_eq!(update_refusal(&uninit, true), None);
    }

    #[test]
    fn plan_selects_paths_and_folders() {
        let subs = vec![
            info("libs/a", Some(2)),
            info("libs/b", Some(1)),
            info("c", None),
        ];
        let request = |paths: Option<&[&str]>, init| SubmoduleUpdate {
            paths: paths.map(|p| p.iter().map(|s| RepoPath::from(*s)).collect()),
            init,
            recursive: false,
        };
        let plan = plan_update(subs.clone(), &request(None, false)).unwrap();
        let paths = |v: &[SubmoduleInfo]| v.iter().map(|i| i.path.to_string()).collect::<Vec<_>>();
        assert_eq!(paths(&plan.run), ["libs/a"]);
        assert_eq!(paths(&plan.unchanged), ["libs/b"]);
        assert_eq!(plan.refused.len(), 1);
        assert_eq!(plan.refused[0].reason, R::NotInitialized);

        let plan = plan_update(subs.clone(), &request(Some(&["libs", "c"]), true)).unwrap();
        assert_eq!(paths(&plan.run), ["libs/a", "c"]);

        // A dirty submodule already on the recorded commit is not refused: nothing would run.
        let mut dirty = info("d", Some(1));
        dirty.changes = true;
        dirty.state = SubmoduleState::LocalChanges;
        let plan = plan_update(vec![dirty], &request(None, false)).unwrap();
        assert_eq!(paths(&plan.unchanged), ["d"]);

        let err = plan_update(subs, &request(Some(&["lib"]), false)).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        assert!(request(Some(&[]), false).check().is_err() && request(None, false).check().is_ok());
    }

    #[test]
    fn nested_results_get_the_prefix() {
        let mut outer = SubmoduleUpdated::default();
        let nested = SubmoduleUpdated {
            unchanged: vec![RepoPath::from("x")],
            refused: vec![SubmoduleRefusal {
                path: RepoPath::from("y"),
                reason: R::LocalChanges,
                message: refusal_message(&RepoPath::from("y"), R::LocalChanges),
            }],
            ..SubmoduleUpdated::default()
        };
        outer.absorb_nested(&RepoPath::from("a b"), nested);
        assert_eq!(outer.unchanged, [RepoPath::from("a b/x")]);
        assert_eq!(outer.refused[0].path, RepoPath::from("a b/y"));
        assert!(outer.refused[0].message.to_string().contains("a b/y"));
    }
}
