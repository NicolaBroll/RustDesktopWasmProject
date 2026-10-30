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
use project_domain::Operation;
pub use project_domain::{HEADS_PREFIX, branch_short};
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
