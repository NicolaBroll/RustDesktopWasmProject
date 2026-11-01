//! Clone and new repository (`repo.clone`, `repo.init`, `impl-docs/git/clone.md`): the requests
//! the [`RepoOpener`](crate::RepoOpener) receives and the checks shared by the openers.

use super::branch::{check_branch_name, check_new_branch_name};
use crate::EngineError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters of `repo.clone`, passed as they are to [`RepoOpener::clone_repo`](crate::RepoOpener::clone_repo).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloneRequest {
    /// What `git clone` accepts: a URL or a local path (never an option, even with `-`).
    pub url: String,
    /// Destination folder: absent or empty.
    pub path: String,
    /// Branch (or tag) to check out; `None` = the remote's HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Shallow clone of this many commits (> 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// Also clone and check out the submodules, recursively (`--recurse-submodules`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub submodules: bool,
}

/// What a clone reports besides the new repo: git's non-progress lines (warnings such as "You
/// appear to have cloned an empty repository.").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneOutcome {
    pub messages: Vec<String>,
}

/// Parameters of `repo.init`, passed to [`RepoOpener::init_repo`](crate::RepoOpener::init_repo).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitRequest {
    /// Destination folder: absent or empty.
    pub path: String,
    /// Initial branch; `None` = git's `init.defaultBranch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

impl CloneRequest {
    /// [`EngineError::InvalidParams`] for an empty `url` or `path`, `depth` 0, or an invalid
    /// `branch` name.
    pub fn check(&self) -> Result<(), EngineError> {
        const ID: &str = "repo.clone";
        let invalid = |reason| EngineError::InvalidParams { id: ID, reason };
        if self.url.is_empty() {
            return Err(invalid(msg!("engine-clone-url-empty")));
        }
        if self.url.contains(['\n', '\0']) {
            return Err(invalid(msg!("engine-clone-url-invalid")));
        }
        check_path(ID, &self.path)?;
        if self.depth == Some(0) {
            return Err(invalid(msg!("engine-clone-depth-zero")));
        }
        if let Some(branch) = &self.branch {
            check_branch_name(ID, branch)?;
        }
        Ok(())
    }
}

impl InitRequest {
    /// [`EngineError::InvalidParams`] for an empty `path` or an invalid new `branch` name.
    pub fn check(&self) -> Result<(), EngineError> {
        const ID: &str = "repo.init";
        check_path(ID, &self.path)?;
        if let Some(branch) = &self.branch {
            check_new_branch_name(ID, branch)?;
        }
        Ok(())
    }
}

pub(super) fn check_path(id: &'static str, path: &str) -> Result<(), EngineError> {
    if path.is_empty() || path.contains('\0') {
        return Err(EngineError::InvalidParams {
            id,
            reason: msg!("engine-clone-path-empty"),
        });
    }
    Ok(())
}

/// The destination exists and is not empty: nothing is written.
pub fn destination_not_empty(path: &str) -> EngineError {
    EngineError::NotAllowed(msg!("engine-clone-not-empty", path = path))
}

/// The destination exists and is not a folder (a file, a symbolic link): nothing is written.
pub fn destination_not_folder(path: &str) -> EngineError {
    EngineError::NotAllowed(msg!("engine-clone-not-folder", path = path))
}

/// The folder that should contain the destination does not exist: nothing is written.
pub fn destination_no_parent(path: &str) -> EngineError {
    EngineError::NotAllowed(msg!("engine-clone-no-parent", path = path))
}

/// The remote does not have the requested branch: nothing left behind.
pub fn clone_no_such_branch(url: &str, branch: &str, message: &str) -> EngineError {
    EngineError::NotAllowed(msg!(
        "engine-clone-no-such-branch",
        url = url,
        branch = branch,
        message = message
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clone(url: &str, path: &str, branch: Option<&str>, depth: Option<u32>) -> CloneRequest {
        CloneRequest {
            url: url.into(),
            path: path.into(),
            branch: branch.map(Into::into),
            depth,
            submodules: false,
        }
    }

    #[test]
    fn checks_the_parameters() {
        assert!(clone("u", "p", Some("main"), Some(1)).check().is_ok());
        // A URL starting with `-` is a URL: the backend puts it after `--end-of-options`.
        assert!(clone("-u", "p", None, None).check().is_ok());
        for bad in [
            clone("", "p", None, None),
            clone("u\nx", "p", None, None),
            clone("u", "", None, None),
            clone("u", "p", None, Some(0)),
            clone("u", "p", Some("a..b"), None),
            clone("u", "p", Some(""), None),
        ] {
            let err = bad.check().unwrap_err();
            assert!(
                matches!(
                    err,
                    EngineError::InvalidParams {
                        id: "repo.clone",
                        ..
                    }
                ),
                "{bad:?}: {err}"
            );
        }
        let init = |branch: Option<&str>| InitRequest {
            path: "p".into(),
            branch: branch.map(Into::into),
        };
        assert!(init(None).check().is_ok());
        assert!(init(Some("trunk")).check().is_ok());
        for bad in ["-x", "HEAD", "a b", ""] {
            assert!(init(Some(bad)).check().is_err(), "{bad}");
        }
    }
}
