//! Pull (`remote.pull`, S26): types and checks shared by the backends
//! (`impl-docs/git/sync.md`).
//!
//! The pull is driven by the engine in two steps: first the fetch of only the upstream's branch
//! ([`GitRemotes::fetch`](super::GitRemotes::fetch), slow and cancellable), then the local
//! integration ([`GitRemotes::pull`](super::GitRemotes::pull): fast-forward, merge or rebase), which
//! cannot be cancelled midway. So the command knows what arrived before writing, decides the
//! strategy ([`PullSettings::policy`], [`pull_mode`]), checks the repo state
//! ([`check_pull_state`]) and records the branch move in the journal.

use super::{HEADS_PREFIX, check_no_operation};
use crate::{EngineError, Message};
use project_domain::remote_short;
use project_domain::{EntryKind, Oid, RepoPath, Status};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The configured upstream of a local branch (`branch.<name>.remote` and `branch.<name>.merge`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamConfig {
    /// The remote (`branch.<name>.remote`); `.` means the repo itself (local upstream).
    pub remote: String,
    /// The branch on the remote, full name (`branch.<name>.merge`, e.g. `refs/heads/main`).
    pub merge: String,
    /// The local remote branch that mirrors it according to the remote's fetch refspec (e.g.
    /// `refs/remotes/origin/main`); `None` if no refspec maps it.
    pub tracking: Option<String>,
}

impl UpstreamConfig {
    /// The short name of the branch on the remote (`main` for `refs/heads/main`), if it is a branch.
    pub fn merge_branch(&self) -> Option<&str> {
        self.merge.strip_prefix(HEADS_PREFIX)
    }
}

/// The pull strategy configuration, with the values as the user writes them in git
/// config (read by the backend, interpreted by [`PullSettings::policy`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullSettings {
    /// `branch.<name>.rebase`, otherwise `pull.rebase` (`None`: neither). A key
    /// without a value is `""` (means `true`).
    pub rebase: Option<String>,
    /// `pull.ff` (`true`, `false`, `only`).
    pub ff: Option<String>,
}

/// The strategy requested with the `strategy` parameter of `remote.pull`, which replaces the
/// configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PullStrategy {
    /// Fast-forward if possible, otherwise a merge commit.
    Merge,
    /// Fast-forward if possible, otherwise rebase the local commits onto the upstream.
    Rebase,
    /// Fast-forward only: if the branches diverged, error and nothing changes.
    FfOnly,
}

/// Where the strategy used comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategySource {
    /// The `strategy` parameter.
    Param,
    /// `pull.rebase`, `branch.<name>.rebase` or `pull.ff`.
    Config,
    /// No configuration: merge (see [`PullSettings::policy`]).
    Default,
}

/// The strategy decided for a pull.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullPolicy {
    pub strategy: PullStrategy,
    /// `pull.ff=false`: even when a fast-forward would be possible a merge commit is created
    /// (only with [`PullStrategy::Merge`]).
    pub no_ff: bool,
    /// `pull.rebase=merges`: rebase with `--rebase-merges`.
    pub rebase_merges: bool,
    pub source: StrategySource,
}

/// A git config boolean value (`true`/`yes`/`on`/`1`, `false`/`no`/`off`/`0`; a key
/// without a value means `true`).
fn config_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "" | "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

impl PullSettings {
    /// The strategy: `explicit` (the `strategy` parameter) if present, otherwise the configuration
    /// as `git pull` reads it:
    ///
    /// - `branch.<name>.rebase` / `pull.rebase`: `true` → rebase, `merges` → rebase with
    ///   `--rebase-merges`, `false` → merge; `interactive` → [`EngineError::NotAllowed`] (an
    ///   interactive rebase needs an editor);
    /// - `pull.ff`: `only` → fast-forward only, `false` → always a merge commit (with the
    ///   merge strategy), `true` → the default.
    ///
    /// Without configuration: **merge** (a choice). Since 2.33 git in that case refuses to
    /// pull diverged branches and asks to configure `pull.rebase`; here we do as git did
    /// before 2.33, because a merge rewrites nothing and is undone with `edit.undo`, and the
    /// result says which strategy was used and where it comes from (`strategy_source`).
    pub fn policy(&self, explicit: Option<PullStrategy>) -> Result<PullPolicy, EngineError> {
        if let Some(strategy) = explicit {
            return Ok(PullPolicy {
                strategy,
                no_ff: false,
                rebase_merges: false,
                source: StrategySource::Param,
            });
        }
        let invalid = |key: &str, value: &str| {
            EngineError::NotAllowed(msg!(
                "engine-pull-config-value",
                key = key,
                value = value.escape_debug()
            ))
        };
        let (mut strategy, mut rebase_merges, mut configured) = (PullStrategy::Merge, false, false);
        if let Some(value) = &self.rebase {
            configured = true;
            match value.to_ascii_lowercase().as_str() {
                "merges" | "m" => {
                    strategy = PullStrategy::Rebase;
                    rebase_merges = true;
                }
                "interactive" | "i" => {
                    return Err(EngineError::NotAllowed(msg!("engine-pull-interactive")));
                }
                other => match config_bool(other) {
                    Some(true) => strategy = PullStrategy::Rebase,
                    Some(false) => {}
                    None => return Err(invalid("pull.rebase", value)),
                },
            }
        }
        let mut no_ff = false;
        if let Some(value) = &self.ff {
            configured = true;
            match value.to_ascii_lowercase().as_str() {
                "only" => strategy = PullStrategy::FfOnly,
                other => match config_bool(other) {
                    Some(true) => {}
                    Some(false) => no_ff = strategy == PullStrategy::Merge,
                    None => return Err(invalid("pull.ff", value)),
                },
            }
        }
        Ok(PullPolicy {
            strategy,
            no_ff,
            rebase_merges,
            source: if configured {
                StrategySource::Config
            } else {
                StrategySource::Default
            },
        })
    }
}

/// How to integrate the upstream into the branch ([`GitRemotes::pull`](super::GitRemotes::pull)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullMode {
    /// The branch advances to the upstream (`git merge --ff-only`, or `update-ref` for a branch that
    /// is not the one of HEAD).
    FastForward,
    /// A merge commit (parents: the branch and the upstream).
    Merge { no_ff: bool },
    /// The local commits rewritten on top of the upstream.
    Rebase { merges: bool },
}

impl PullMode {
    /// The name in the result (`strategy`): `fast_forward`, `merge`, `rebase`.
    pub fn name(&self) -> &'static str {
        match self {
            PullMode::FastForward => "fast_forward",
            PullMode::Merge { .. } => "merge",
            PullMode::Rebase { .. } => "rebase",
        }
    }
}

/// What to do given the branch's commits the upstream does not have (`ahead`) and the upstream's
/// commits the branch does not have (`behind`): `None` if the branch already contains the upstream (nothing to
/// do). A branch that is not the one of HEAD (`current == false`) can only be moved forward
/// (fast-forward), without touching the worktree; diverged branches with `ff_only` →
/// [`EngineError::NotAllowed`].
pub fn pull_mode(
    policy: &PullPolicy,
    branch: &str,
    current: bool,
    ahead: usize,
    behind: usize,
) -> Result<Option<PullMode>, EngineError> {
    if behind == 0 {
        return Ok(None);
    }
    if ahead == 0 {
        if current && policy.strategy == PullStrategy::Merge && policy.no_ff {
            return Ok(Some(PullMode::Merge { no_ff: true }));
        }
        return Ok(Some(PullMode::FastForward));
    }
    if !current {
        return Err(EngineError::NotAllowed(msg!(
            "engine-pull-diverged-not-current",
            branch = branch.escape_debug(),
            ahead = ahead,
            behind = behind
        )));
    }
    match policy.strategy {
        PullStrategy::FfOnly => Err(EngineError::NotAllowed(msg!(
            "engine-pull-diverged-ff-only",
            branch = branch.escape_debug(),
            ahead = ahead,
            behind = behind
        ))),
        PullStrategy::Rebase => Ok(Some(PullMode::Rebase {
            merges: policy.rebase_merges,
        })),
        PullStrategy::Merge => Ok(Some(PullMode::Merge {
            no_ff: policy.no_ff,
        })),
    }
}

/// The checks before integrating the upstream, the same for the backends, on the state read now:
///
/// - merge, rebase, cherry-pick, revert, bisect or `git am` in progress → [`EngineError::NotAllowed`];
/// - conflicted files → [`EngineError::NotAllowed`];
/// - for merge and rebase, **any** change to tracked files (staged or not) →
///   [`EngineError::NotAllowed`]: a merge or rebase that stops on conflict with
///   local changes cannot be aborted safely (`git merge --abort` may not rebuild them)
///   and a rebase does not start. Never `--autostash`: changes must be committed or set aside
///   first. Untracked files are fine (git refuses to overwrite them). Fast-forward instead
///   accepts changes that do not touch the changed files: git itself refuses to overwrite the
///   others.
pub fn check_pull_state(status: &Status, mode: &PullMode) -> Result<(), EngineError> {
    check_no_operation("remote.pull", msg!("engine-busy-pull"), status.operation)?;
    let unmerged: Vec<String> = status
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
        .map(|e| format!("`{}`", e.path))
        .collect();
    if !unmerged.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-pull-conflicts",
            files = unmerged.join(", ")
        )));
    }
    if matches!(mode, PullMode::FastForward) {
        return Ok(());
    }
    let changed: Vec<String> = status
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. }
            ) && (e.index.is_change() || e.worktree.is_change())
        })
        .map(|e| format!("`{}`", e.path))
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    Err(EngineError::NotAllowed(msg!(
        "engine-pull-local-changes",
        mode = mode.name(),
        files = changed.join(", ")
    )))
}

/// The message of the merge commit of a pull: like `git merge <remote branch>`
/// (`Merge remote-tracking branch 'origin/main'`), with ` into <branch>` if the branch is not
/// `main` or `master` (as git does with the default `merge.suppressDest`).
pub fn merge_message(upstream: &str, branch: &str) -> String {
    let short = remote_short(upstream);
    let mut message = format!("Merge remote-tracking branch '{short}'");
    if branch != "main" && branch != "master" {
        message.push_str(&format!(" into {branch}"));
    }
    message
}

/// What `remote.pull` asks of the backend after the fetch ([`GitRemotes::pull`](super::GitRemotes::pull)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// The local branch (short name).
    pub branch: String,
    /// True if it is the branch of HEAD (index and worktree are updated); otherwise only
    /// fast-forward of the ref.
    pub current: bool,
    /// The value of the branch read earlier: the backend refuses if it changed.
    pub expected: Oid,
    /// The upstream commit to integrate (the remote branch after the fetch).
    pub upstream: Oid,
    /// The full name of the remote branch (e.g. `refs/remotes/origin/main`), for messages.
    pub upstream_name: String,
    pub mode: PullMode,
    /// The message of the merge commit ([`merge_message`]).
    pub message: String,
}

/// Outcome of [`GitRemotes::pull`](super::GitRemotes::pull).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullOutcome {
    /// The value of the branch afterwards; if the merge is stopped ([`PullOutcome::in_progress`]) the one
    /// from before.
    pub after: Oid,
    /// The backup ref of the branch before the pull (`refs/project/backup/pull-...`); it stays
    /// even if the merge is stopped.
    pub backup: String,
    /// True if the merge or the rebase stopped (conflicts, or a hook or the signing that failed)
    /// and **stays in progress**, like `merge.start`/`rebase.start`: to be concluded with
    /// `merge.continue`/`rebase.continue` or aborted with `merge.abort`/`rebase.abort`.
    pub in_progress: bool,
    /// The conflicted files of the stopped merge or rebase: those still unmerged and those git reported
    /// as conflicted (even if `rerere` already resolved them); empty otherwise.
    pub conflicts: Vec<RepoPath>,
    /// The git text (stdout and stderr) of the stopped merge or rebase; empty otherwise.
    pub message: String,
    /// Warnings found after a successful integration (e.g. a hook that moved HEAD).
    pub warnings: Vec<Message>,
}

/// Which operation stopped on conflict ([`EngineError::PullConflict`]). No backend produces the
/// error any more: a stopped merge or rebase stays in progress ([`PullOutcome::in_progress`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullOperation {
    Merge,
    Rebase,
}

impl std::fmt::Display for PullOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PullOperation::Merge => "merge",
            PullOperation::Rebase => "rebase",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Change, Head, RepoPath, StatusEntry, TrackedInfo};

    fn settings(rebase: Option<&str>, ff: Option<&str>) -> PullSettings {
        PullSettings {
            rebase: rebase.map(str::to_owned),
            ff: ff.map(str::to_owned),
        }
    }

    #[test]
    fn policy_from_config() {
        let p = settings(None, None).policy(None).unwrap();
        assert_eq!(
            (p.strategy, p.no_ff, p.source),
            (PullStrategy::Merge, false, StrategySource::Default)
        );
        for v in ["true", "yes", "on", "1", "", "TRUE"] {
            let p = settings(Some(v), None).policy(None).unwrap();
            assert_eq!(p.strategy, PullStrategy::Rebase, "{v}");
            assert_eq!(p.source, StrategySource::Config);
        }
        let p = settings(Some("merges"), None).policy(None).unwrap();
        assert!(p.rebase_merges && p.strategy == PullStrategy::Rebase);
        let p = settings(Some("false"), Some("false")).policy(None).unwrap();
        assert!(p.no_ff && p.strategy == PullStrategy::Merge);
        let p = settings(Some("true"), Some("only")).policy(None).unwrap();
        assert_eq!(p.strategy, PullStrategy::FfOnly);
        let p = settings(Some("true"), Some("false")).policy(None).unwrap();
        assert!(!p.no_ff, "pull.ff=false only matters for the merge");
        for (rebase, ff) in [
            (Some("interactive"), None),
            (Some("x"), None),
            (None, Some("x")),
        ] {
            let err = settings(rebase, ff).policy(None).unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        }
        // The parameter replaces the configuration, even if not valid.
        let p = settings(Some("interactive"), Some("only"))
            .policy(Some(PullStrategy::Merge))
            .unwrap();
        assert_eq!(
            (p.strategy, p.no_ff, p.source),
            (PullStrategy::Merge, false, StrategySource::Param)
        );
    }

    #[test]
    fn mode_from_ahead_and_behind() {
        let policy = |strategy, no_ff| PullPolicy {
            strategy,
            no_ff,
            rebase_merges: false,
            source: StrategySource::Default,
        };
        let merge = policy(PullStrategy::Merge, false);
        assert_eq!(pull_mode(&merge, "main", true, 3, 0).unwrap(), None);
        assert_eq!(
            pull_mode(&merge, "main", true, 0, 2).unwrap(),
            Some(PullMode::FastForward)
        );
        assert_eq!(
            pull_mode(&merge, "main", true, 1, 2).unwrap(),
            Some(PullMode::Merge { no_ff: false })
        );
        assert_eq!(
            pull_mode(&policy(PullStrategy::Merge, true), "main", true, 0, 2).unwrap(),
            Some(PullMode::Merge { no_ff: true })
        );
        // An inactive branch: fast-forward only, even with `no_ff`.
        assert_eq!(
            pull_mode(&policy(PullStrategy::Merge, true), "x", false, 0, 2).unwrap(),
            Some(PullMode::FastForward)
        );
        assert!(matches!(
            pull_mode(&merge, "x", false, 1, 2),
            Err(EngineError::NotAllowed(_))
        ));
        assert_eq!(
            pull_mode(&policy(PullStrategy::Rebase, false), "main", true, 1, 2).unwrap(),
            Some(PullMode::Rebase { merges: false })
        );
        assert!(matches!(
            pull_mode(&policy(PullStrategy::FfOnly, false), "main", true, 1, 2),
            Err(EngineError::NotAllowed(_))
        ));
    }

    fn entry(path: &str, index: Change, worktree: Change, kind: EntryKind) -> StatusEntry {
        StatusEntry {
            path: RepoPath::from(path),
            index,
            worktree,
            kind,
        }
    }

    #[test]
    fn state_checks() {
        let info = TrackedInfo {
            submodule: None,
            mode_head: 0o100644,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: Oid::from_bytes([1; Oid::LEN]),
            oid_index: Oid::from_bytes([1; Oid::LEN]),
        };
        let mut status = Status {
            head: Head::Branch {
                name: "main".into(),
                target: None,
            },
            upstream: None,
            entries: vec![entry(
                "new",
                Change::Untracked,
                Change::Untracked,
                EntryKind::Untracked,
            )],
            operation: None,
        };
        for mode in [
            PullMode::FastForward,
            PullMode::Merge { no_ff: false },
            PullMode::Rebase { merges: false },
        ] {
            check_pull_state(&status, &mode).unwrap();
        }
        status.entries.push(entry(
            "a",
            Change::Unmodified,
            Change::Modified,
            EntryKind::Ordinary(info),
        ));
        check_pull_state(&status, &PullMode::FastForward).unwrap();
        let err = check_pull_state(&status, &PullMode::Rebase { merges: false }).unwrap_err();
        assert!(err.to_string().contains("`a`"), "{err}");
        status.entries.pop();
        status.operation = Some(project_domain::Operation::Merge);
        assert!(check_pull_state(&status, &PullMode::FastForward).is_err());
    }

    #[test]
    fn merge_messages() {
        assert_eq!(
            merge_message("refs/remotes/origin/main", "main"),
            "Merge remote-tracking branch 'origin/main'"
        );
        assert_eq!(
            merge_message("refs/remotes/origin/x", "x"),
            "Merge remote-tracking branch 'origin/x' into x"
        );
    }
}
