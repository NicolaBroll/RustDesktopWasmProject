//! Local branches (`branch.create`, `branch.checkout`, `branch.delete`, `branch.rename`, S24):
//! checks shared by the backends (`impl-docs/git/branch.md`).
//!
//! The names passed to the commands are **short** (`feature/x`, not `refs/heads/feature/x`); the
//! full name is built by [`branch_ref`]. A new name (creation, rename) must be a
//! branch name valid for git ([`check_new_branch_name`]: no leading `-`, no `HEAD`,
//! the rules of `git check-ref-format`); an existing branch can be named even if the name
//! would no longer be accepted today (e.g. `-x`, created with `update-ref`), as long as it is a valid ref name
//! ([`check_branch_name`]): so it can still be renamed or deleted.

use super::valid_ref_name;
use crate::{EngineError, Message};
pub use project_domain::{HEADS_PREFIX, branch_short};
use project_domain::{Oid, Operation, REMOTES_PREFIX, RefInfo, RefKind};
use serde::{Deserialize, Serialize};

/// The full name of the local branch `name` (`refs/heads/<name>`).
pub fn branch_ref(name: &str) -> String {
    format!("{HEADS_PREFIX}{name}")
}

/// The name of an existing branch (`branch.checkout`, `branch.delete`, the `name` of
/// `branch.rename`): a short name that gives a valid ref name ([`valid_ref_name`]). A name
/// starting with `-` is accepted (it may exist, created with `update-ref`): backends always receive it
/// after `--end-of-options` or as `refs/heads/<name>`. Otherwise
/// [`EngineError::InvalidParams`].
pub fn check_branch_name(id: &'static str, name: &str) -> Result<(), EngineError> {
    let reason = if name.is_empty() {
        Some(msg!("engine-branch-name-empty"))
    } else if name.starts_with("refs/") {
        Some(msg!("engine-branch-name-full"))
    } else if !valid_ref_name(&branch_ref(name)) {
        Some(msg!("engine-branch-name-invalid"))
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EngineError::InvalidParams {
            id,
            reason: msg!(
                "engine-with-name",
                reason = reason,
                name = name.escape_debug().to_string()
            ),
        }),
        None => Ok(()),
    }
}

/// The name of a **new** branch (`branch.create`, the `new_name` of `branch.rename`): like
/// [`check_branch_name`] plus the rules of `git check-ref-format --branch`, which git
/// applies to new names: it does not start with `-`, and is not `HEAD` or `@`. The `CliBackend` also has
/// git verify it before writing. Otherwise [`EngineError::InvalidParams`].
pub fn check_new_branch_name(id: &'static str, name: &str) -> Result<(), EngineError> {
    check_branch_name(id, name)?;
    let reason = if name.starts_with('-') {
        Some(msg!("engine-branch-name-dash"))
    } else if name == "HEAD" || name == "@" {
        Some(msg!("engine-branch-name-reserved"))
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EngineError::InvalidParams {
            id,
            reason: msg!(
                "engine-with-name",
                reason = reason,
                name = name.escape_debug().to_string()
            ),
        }),
        None => Ok(()),
    }
}

/// [`EngineError::NotAllowed`] if an operation is in progress (merge, rebase, cherry-pick, revert,
/// bisect, `git am`): `command` `what` (e.g. msg!("engine-busy-checkout")) in the middle of the operation.
pub fn check_no_operation(
    command: &str,
    what: Message,
    operation: Option<Operation>,
) -> Result<(), EngineError> {
    let Some(op) = operation else {
        return Ok(());
    };
    // The selector of `engine-operation-in-progress`.
    let operation = match op {
        Operation::Merge => "merge",
        Operation::Rebase => "rebase",
        Operation::CherryPick => "cherry-pick",
        Operation::Revert => "revert",
        Operation::Bisect => "bisect",
        Operation::Am => "am",
        Operation::Squash => "squash",
    };
    Err(EngineError::NotAllowed(msg!(
        "engine-operation-in-progress",
        operation = operation,
        command = command,
        what = what
    )))
}

/// Outcome of [`GitRefs::delete_branch`](crate::GitRefs::delete_branch).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchDeleted {
    /// The backup ref on the branch's commit (`refs/project/backup/delete-...`), created before
    /// deleting it.
    pub backup: String,
    /// The repo's `branch.<name>.*` configuration removed with the branch (upstream,
    /// description, ...), in file order: `edit.undo` writes it back.
    pub config: Vec<ConfigEntry>,
    /// Warnings about something that failed **after** the deletion (e.g. the `branch.<name>.*`
    /// configuration not removed, or a non-UTF-8 value not saved). Usually empty.
    pub warnings: Vec<Message>,
}

/// An entry of the repo configuration, e.g. `{"key": "branch.x.remote", "value":
/// "origin"}`. `value` is `null` for a key without a value (`[branch "x"] flag`, means `true`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEntry {
    pub key: String,
    pub value: Option<String>,
}

/// A remote-tracking branch `refs/remotes/<remote>/<branch>` of a configured remote, which a
/// local branch can take as upstream (`branch.create` with `track`, `branch.set_upstream`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackingBranch {
    /// The configured remote (may contain `/`).
    pub remote: String,
    /// The branch on the remote, short name (`branch.<name>.merge` is `refs/heads/<branch>`).
    pub branch: String,
    /// The full name of the remote-tracking branch.
    pub full: String,
    /// The commit it points to.
    pub target: Oid,
}

impl TrackingBranch {
    /// `<remote>/<branch>`, as the UI and the results show it.
    pub fn label(&self) -> String {
        format!("{}/{}", self.remote, self.branch)
    }

    /// The `branch.<local>.remote` and `branch.<local>.merge` entries that make it the upstream
    /// of the local branch `local` (what `git branch --track` writes).
    pub fn entries(&self, local: &str) -> Vec<ConfigEntry> {
        vec![
            ConfigEntry {
                key: format!("branch.{local}.remote"),
                value: Some(self.remote.clone()),
            },
            ConfigEntry {
                key: format!("branch.{local}.merge"),
                value: Some(branch_ref(&self.branch)),
            },
        ]
    }
}

/// The remote-tracking branch `spec` (`<remote>/<branch>` or `refs/remotes/<remote>/<branch>`)
/// among `refs`: it must exist (not a symbolic ref like `origin/HEAD`) and belong to exactly one
/// of the configured `remotes`. With remotes `a` and `a/b`, `a/b/c` could be the branch `b/c` of
/// `a` or `c` of `a/b`: refused as ambiguous (git refuses it too). Otherwise
/// [`EngineError::InvalidParams`].
pub fn resolve_tracking(
    id: &'static str,
    spec: &str,
    refs: &[RefInfo],
    remotes: &[String],
) -> Result<TrackingBranch, EngineError> {
    let invalid = |reason: Message| EngineError::InvalidParams { id, reason };
    let full = if spec.starts_with("refs/") {
        spec.to_owned()
    } else {
        format!("{REMOTES_PREFIX}{spec}")
    };
    let Some(found) = refs
        .iter()
        .find(|r| r.name == full && r.kind == RefKind::RemoteBranch)
    else {
        return Err(invalid(msg!(
            "engine-upstream-not-remote-branch",
            spec = spec.escape_debug()
        )));
    };
    let rest = &full[REMOTES_PREFIX.len()..];
    let owners: Vec<(&String, &str)> = remotes
        .iter()
        .filter_map(|r| {
            let branch = rest.strip_prefix(r.as_str())?.strip_prefix('/')?;
            (!branch.is_empty() && branch != "HEAD").then_some((r, branch))
        })
        .collect();
    match owners.as_slice() {
        [(remote, branch)] => Ok(TrackingBranch {
            remote: (*remote).clone(),
            branch: (*branch).to_owned(),
            full: full.clone(),
            target: found.target,
        }),
        [] => Err(invalid(msg!(
            "engine-upstream-no-remote",
            spec = spec.escape_debug()
        ))),
        many => Err(invalid(msg!(
            "engine-upstream-ambiguous",
            spec = spec.escape_debug(),
            remotes = many
                .iter()
                .map(|(r, _)| format!("`{r}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// The upstream of a local branch as the repo file stores it (`branch.<name>.remote` and
/// `branch.<name>.merge`), shown as `<remote>/<branch>` (`<remote>:<merge>` if `merge` is not
/// a branch); `None` without entries.
pub fn upstream_label(entries: &[ConfigEntry]) -> Option<String> {
    let value = |suffix: &str| {
        entries
            .iter()
            .find(|e| e.key.ends_with(suffix))
            .map(|e| e.value.clone().unwrap_or_default())
    };
    match (value(".remote"), value(".merge")) {
        (None, None) => None,
        (Some(remote), Some(merge)) => Some(match merge.strip_prefix(HEADS_PREFIX) {
            Some(branch) => format!("{remote}/{branch}"),
            None => format!("{remote}:{merge}"),
        }),
        (Some(remote), None) => Some(remote),
        (None, Some(merge)) => Some(merge),
    }
}

/// The upstream configuration of a local branch changed by `branch.create` (with `track`) or
/// `branch.set_upstream`: the `branch.<name>.remote` / `branch.<name>.merge` entries of the repo
/// file before and after, in file order. Undo writes `before` back, redo `after`, only if the
/// repo still has the other one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamChange {
    /// The local branch, short name.
    pub branch: String,
    pub before: Vec<ConfigEntry>,
    pub after: Vec<ConfigEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for good in ["main", "feature/x-1", "-x", "a.b", "über"] {
            check_branch_name("t", good).unwrap();
        }
        for bad in [
            "",
            "refs/heads/main",
            "a b",
            "a..b",
            "a@{1}",
            ".x",
            "x.lock",
            "x/",
            "a//b",
            "a:b",
            "a\nb",
        ] {
            assert!(
                matches!(
                    check_branch_name("t", bad),
                    Err(EngineError::InvalidParams { .. })
                ),
                "{bad:?}"
            );
        }
        check_new_branch_name("t", "feature/x").unwrap();
        for bad in ["-x", "--force", "HEAD", "@", "a b", ""] {
            assert!(check_new_branch_name("t", bad).is_err(), "{bad:?}");
        }
        assert_eq!(branch_ref("x"), "refs/heads/x");
        assert_eq!(branch_short("refs/heads/a/b"), "a/b");
        assert_eq!(branch_short("refs/tags/v1"), "refs/tags/v1");
    }

    #[test]
    fn operations() {
        check_no_operation("branch.checkout", msg!("engine-busy-checkout"), None).unwrap();
        let err = check_no_operation(
            "branch.checkout",
            msg!("engine-busy-checkout"),
            Some(Operation::Rebase),
        )
        .unwrap_err();
        assert!(err.to_string().contains("a rebase"), "{err}");
    }
}
