//! Git LFS reads (S63, `impl-docs/git/reading.md` "Git LFS"): `repo.lfs_paths` (which files are
//! stored in LFS: the UI marks them and hides line operations) and `repo.lfs_status` (a repo that
//! uses LFS without git-lfs installed: the UI shows a notice).

use super::{NoParams, Out, check_path, to_json};
use crate::Session;
use project_domain::RepoPath;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters of `repo.lfs_paths`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LfsPathsParams {
    /// The files to check (e.g. those of a diff or of the status); may be empty.
    pub paths: Vec<RepoPath>,
}

/// Result of `repo.lfs_paths`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LfsPathsResult {
    /// The requested paths whose `filter` attribute is `lfs`, in the order given, each once.
    pub paths: Vec<RepoPath>,
}

/// Which of `paths` are stored in Git LFS ([`GitRead::lfs_paths`](crate::GitRead::lfs_paths)).
///
/// Result: [`LfsPathsResult`].
pub(super) fn repo_lfs_paths(s: &mut Session, LfsPathsParams { paths }: LfsPathsParams) -> Out {
    let paths = paths
        .into_iter()
        .map(|p| check_path("repo.lfs_paths", p))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(to_json(LfsPathsResult {
        paths: s.repo()?.lfs_paths(&paths)?,
    }))
}

/// Whether the repo uses LFS and git-lfs is installed
/// ([`GitRead::lfs_state`](crate::GitRead::lfs_state)).
///
/// Result: [`LfsState`](crate::LfsState).
pub(super) fn repo_lfs_status(s: &mut Session, NoParams {}: NoParams) -> Out {
    Ok(to_json(s.repo()?.lfs_state()?))
}

#[cfg(test)]
mod tests {
    use crate::{MemoryBackend, MemoryOpener, Registry, Session};
    use project_domain::RepoPath;
    use serde_json::json;
    use std::sync::Arc;

    fn session(backend: MemoryBackend) -> (Registry, Session) {
        let r = Registry::builtin();
        let mut opener = MemoryOpener::default();
        opener.insert("r", backend);
        let mut s = Session::new(Arc::new(opener));
        r.run("repo.open", &mut s, json!({"path": "r"})).unwrap();
        (r, s)
    }

    #[test]
    fn lfs_paths_and_status() {
        let mut backend = MemoryBackend::new();
        backend.set_lfs([RepoPath::from("a.bin"), RepoPath::from("d/b.bin")], false);
        let (r, mut s) = session(backend);
        let out = r
            .run(
                "repo.lfs_paths",
                &mut s,
                json!({"paths": ["x.txt", "d/b.bin", "a.bin", "d/b.bin"]}),
            )
            .unwrap();
        assert_eq!(out, json!({"paths": ["d/b.bin", "a.bin"]}));
        let out = r
            .run("repo.lfs_paths", &mut s, json!({"paths": []}))
            .unwrap();
        assert_eq!(out, json!({"paths": []}));
        let out = r.run("repo.lfs_status", &mut s, json!({})).unwrap();
        assert_eq!(out, json!({"used": true, "installed": false}));
    }

    #[test]
    fn lfs_paths_refuses_bad_paths() {
        let (r, mut s) = session(MemoryBackend::new());
        for path in ["", "../x", "a//b"] {
            let err = r
                .run("repo.lfs_paths", &mut s, json!({"paths": [path]}))
                .unwrap_err();
            assert!(
                matches!(err, crate::EngineError::InvalidParams { .. }),
                "{path}: {err}"
            );
        }
        let out = r.run("repo.lfs_status", &mut s, json!({})).unwrap();
        assert_eq!(out, json!({"used": false, "installed": true}));
    }
}
