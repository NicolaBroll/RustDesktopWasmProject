//! Remotes (`remote.fetch`, S25): types and checks shared by the backends
//! (`impl-docs/git/sync.md`).
//!
//! A remote is always given by the **name** of a configured remote (`origin`), never by a
//! URL or a path: `git fetch <x>` with an `x` that is not a remote would use it as a URL, so
//! the command checks that the name is among those of
//! [`GitRemotes::remotes`](super::GitRemotes::remotes)
//! before calling the backend.

use super::valid_ref_name;
use crate::EngineError;
use project_domain::REMOTES_PREFIX;
use project_domain::Status;
use serde::{Deserialize, Serialize};

/// What to download with [`GitRemotes::fetch`](super::GitRemotes::fetch). Names already checked by
/// the
/// command: `remote` is a configured remote, `branch` a valid short branch name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchRequest {
    /// The name of the configured remote.
    pub remote: String,
    /// Only this branch of the remote (short name, e.g. `main`); `None`: what the remote's
    /// configuration says (`remote.<name>.fetch`, usually all branches).
    pub branch: Option<String>,
    /// Also remove the remote-tracking branches whose branch no longer exists on the remote
    /// (`--prune`, only with `branch: None`). `false`: as the configuration says (`fetch.prune`).
    #[serde(default)]
    pub prune: bool,
}

/// The result of [`GitRemotes::fetch`](super::GitRemotes::fetch). The updated refs are computed by
/// the command comparing the refs before and after.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchOutcome {
    /// The git lines that are not progress (`From <url>`, ` * [new branch] main ->
    /// origin/main`, warnings), in order.
    pub messages: Vec<String>,
    /// The backup refs (`refs/project/backup/prune-...`) of the removed remote-tracking branches
    /// whose commits no other ref reaches: the remote no longer has them, the repo keeps them here.
    #[serde(default)]
    pub backups: Vec<String>,
}

/// The name of a remote passed to `remote.*`: not empty, without control characters, and such
/// that `refs/remotes/<name>/x` is a valid ref name (the same check git does for remote
/// names). A name starting with `-` is accepted (git allows it): backends always receive it
/// after `--end-of-options`. Otherwise [`EngineError::InvalidParams`].
pub fn check_remote_name(id: &'static str, name: &str) -> Result<(), EngineError> {
    let reason = if name.is_empty() {
        Some(msg!("engine-remote-name-empty"))
    } else if name.chars().any(char::is_control) {
        Some(msg!("engine-remote-name-control"))
    } else if !valid_ref_name(&format!("{REMOTES_PREFIX}{name}/x")) {
        Some(msg!("engine-remote-name-invalid"))
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EngineError::InvalidParams {
            id,
            reason: msg!(
                "engine-with-name",
                reason = reason,
                name = name.escape_debug()
            ),
        }),
        None => Ok(()),
    }
}

/// The remote to use when `remote` is missing, like `git fetch` without arguments: the one
/// of the active branch's upstream (`origin/main` → `origin`; with several remotes that could
/// match, the longest name), otherwise `origin`, otherwise the only configured
/// remote. None of these: [`EngineError::NotAllowed`] asking to specify it.
pub fn default_remote(
    id: &'static str,
    status: &Status,
    remotes: &[String],
) -> Result<String, EngineError> {
    let from_upstream = status.upstream.as_ref().and_then(|upstream| {
        remotes
            .iter()
            .filter(|r| {
                upstream
                    .name
                    .strip_prefix(r.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|r| r.len())
    });
    if let Some(remote) = from_upstream {
        return Ok(remote.clone());
    }
    if remotes.iter().any(|r| r == "origin") {
        return Ok("origin".to_owned());
    }
    match remotes {
        [only] => Ok(only.clone()),
        [] => Err(EngineError::NotAllowed(msg!("engine-remote-none", id = id))),
        _ => Err(EngineError::NotAllowed(msg!(
            "engine-remote-which",
            id = id,
            names = names(remotes)
        ))),
    }
}

/// [`EngineError::NotAllowed`] for a remote that is not configured: nothing downloaded.
pub fn unknown_remote(remote: &str, remotes: &[String]) -> EngineError {
    let known = if remotes.is_empty() {
        msg!("engine-remote-none-configured")
    } else {
        msg!("engine-remote-configured", names = names(remotes))
    };
    EngineError::NotAllowed(msg!(
        "engine-remote-unknown",
        remote = remote.escape_debug(),
        known = known
    ))
}

fn names(remotes: &[String]) -> String {
    remotes
        .iter()
        .map(|r| format!("`{}`", r.escape_debug()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Head, Upstream};

    fn status(upstream: Option<&str>) -> Status {
        Status {
            head: Head::Branch {
                name: "main".into(),
                target: None,
            },
            upstream: upstream.map(|name| Upstream {
                name: name.into(),
                ahead: Some(0),
                behind: Some(0),
            }),
            entries: Vec::new(),
            operation: None,
        }
    }

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn remote_names() {
        for ok in ["origin", "-x", "--upload-pack=evil", "a/b", "fork.mine"] {
            check_remote_name("remote.fetch", ok).unwrap();
        }
        for bad in [
            "", "a..b", "a b", "x\n", "a:b", "x~1", "/x", "x/", "@{u}", "a\0b",
        ] {
            let err = check_remote_name("remote.fetch", bad).unwrap_err();
            assert!(matches!(err, EngineError::InvalidParams { .. }), "{bad:?}");
        }
    }

    #[test]
    fn default_remote_follows_the_upstream_then_origin() {
        let id = "remote.fetch";
        let remotes = list(&["fork", "fork/a", "origin"]);
        assert_eq!(
            default_remote(id, &status(Some("fork/main")), &remotes).unwrap(),
            "fork"
        );
        // The longest name wins: `fork/a/main` belongs to the remote `fork/a`.
        assert_eq!(
            default_remote(id, &status(Some("fork/a/main")), &remotes).unwrap(),
            "fork/a"
        );
        // Local upstream (`branch.<b>.remote = .`) or absent: `origin`.
        assert_eq!(
            default_remote(id, &status(Some("other")), &remotes).unwrap(),
            "origin"
        );
        assert_eq!(
            default_remote(id, &status(None), &remotes).unwrap(),
            "origin"
        );
        // Without `origin`: the only remote, or an error if there are several or none.
        assert_eq!(
            default_remote(id, &status(None), &list(&["only"])).unwrap(),
            "only"
        );
        let err = default_remote(id, &status(None), &list(&["a", "b"])).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        let err = default_remote(id, &status(None), &[]).unwrap_err();
        assert!(err.to_string().contains("has no remotes"), "{err}");
    }

    #[test]
    fn unknown_remote_lists_the_configured_ones() {
        let err = unknown_remote("x", &list(&["origin"]));
        assert!(err.to_string().contains("`origin`"), "{err}");
        assert!(matches!(err, EngineError::NotAllowed(_)));
    }
}
