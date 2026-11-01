//! Submodule commands (`impl-docs/git/submodules.md`): `submodule.list` (read) and
//! `submodule.update` (write, slow: progress and cancellation from [`Session::job`]). No journal
//! entry: no ref of the superproject moves; the HEAD a submodule had is in the result and, when
//! it was detached, in a backup ref inside the submodule.

use super::{NoParams, Out, to_json};
use crate::{EngineError, Session, SubmoduleInfo, SubmoduleUpdate};
use serde::{Deserialize, Serialize};

/// Result of `submodule.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmoduleListResult {
    /// In path order; empty if the repo has none.
    pub submodules: Vec<SubmoduleInfo>,
}

/// The submodules of the open repo with their state.
///
/// Result: [`SubmoduleListResult`].
pub(super) fn submodule_list(s: &mut Session, NoParams {}: NoParams) -> Out {
    Ok(to_json(SubmoduleListResult {
        submodules: s.repo()?.submodules()?,
    }))
}

/// Checks out the recorded commit in the selected submodules, except those refused.
///
/// Result: [`SubmoduleUpdated`](crate::SubmoduleUpdated).
pub(super) fn submodule_update(s: &mut Session, request: SubmoduleUpdate) -> Out {
    const ID: &str = "submodule.update";
    request.check()?;
    let job = s.job().clone();
    let repo = s.repo_mut()?;
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    Ok(to_json(repo.update_submodules(&request, &job)?))
}

#[cfg(test)]
mod tests {
    use crate::{
        EngineError, MemoryBackend, MemoryOpener, Registry, Session, SubmoduleInfo, SubmoduleState,
        submodule_state,
    };
    use project_domain::{Oid, RepoPath};
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn oid(n: u8) -> Oid {
        Oid::from_bytes([n; Oid::LEN])
    }

    fn sub(path: &str, head: Option<u8>, f: impl FnOnce(&mut SubmoduleInfo)) -> SubmoduleInfo {
        let head = head.map(oid);
        let mut info = SubmoduleInfo {
            path: RepoPath::from(path),
            name: Some(path.to_owned()),
            url: Some(format!("../{path}")),
            recorded: Some(oid(1)),
            head,
            detached: true,
            state: SubmoduleState::InSync,
            changes: false,
            untracked: false,
            operation: None,
            unreferenced: false,
            ignored_overwritten: false,
        };
        f(&mut info);
        info.state = submodule_state(
            info.recorded.is_none(),
            info.head.is_some(),
            info.changes || info.untracked,
            info.head,
            info.recorded,
        );
        info
    }

    fn setup(subs: Vec<SubmoduleInfo>) -> (Registry, Session) {
        let mut repo = MemoryBackend::sample();
        repo.set_submodules(subs);
        let mut opener = MemoryOpener::new();
        opener.insert("repo", repo);
        let mut s = Session::new(Arc::new(opener));
        s.open("repo").unwrap();
        (Registry::builtin(), s)
    }

    fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Result<Value, EngineError> {
        r.run(id, s, params)
    }

    #[test]
    fn list_reports_the_states() {
        let (r, mut s) = setup(vec![
            sub("b", Some(2), |_| {}),
            sub("a", None, |_| {}),
            sub("c", Some(1), |i| i.untracked = true),
        ]);
        let out = run(&r, &mut s, "submodule.list", json!({})).unwrap();
        let states: Vec<_> = out["submodules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (s["path"].as_str().unwrap(), s["state"].as_str().unwrap()))
            .collect();
        assert_eq!(
            states,
            [
                ("a", "uninitialized"),
                ("b", "different_commit"),
                ("c", "local_changes")
            ]
        );
        assert!(run(&r, &mut s, "submodule.list", json!({"x": 1})).is_err());
    }

    #[test]
    fn update_moves_only_the_safe_ones_and_saves_a_detached_head() {
        let (r, mut s) = setup(vec![
            sub("clean", Some(2), |_| {}),
            sub("on-branch", Some(3), |i| i.detached = false),
            sub("dirty", Some(2), |i| i.changes = true),
            sub("untracked", Some(2), |i| i.untracked = true),
            sub("merging", Some(2), |i| {
                i.operation = Some(project_domain::Operation::Merge)
            }),
            sub("lost", Some(2), |i| i.unreferenced = true),
            sub("dash", Some(2), |i| i.url = Some("-u".into())),
            sub("synced", Some(1), |i| i.untracked = true),
            sub("new", None, |_| {}),
        ]);
        let out = run(&r, &mut s, "submodule.update", json!({})).unwrap();
        let updated: Vec<_> = out["updated"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["path"].as_str().unwrap())
            .collect();
        assert_eq!(updated, ["clean", "on-branch"]);
        assert_eq!(out["updated"][0]["from"], oid(2).to_string());
        assert_eq!(out["updated"][0]["to"], oid(1).to_string());
        assert_eq!(
            out["updated"][0]["backup"],
            "refs/project/backup/submodule-1"
        );
        assert_eq!(out["updated"][1]["backup"], Value::Null);
        assert_eq!(out["unchanged"], json!(["synced"]));
        let refused: Vec<_> = out["refused"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["path"].as_str().unwrap(), r["reason"].as_str().unwrap()))
            .collect();
        assert_eq!(
            refused,
            [
                ("dash", "suspicious_url"),
                ("dirty", "local_changes"),
                ("lost", "unreferenced_commits"),
                ("merging", "operation_in_progress"),
                ("new", "not_initialized"),
                ("untracked", "untracked_files"),
            ]
        );
        assert!(
            out["refused"][1]["message"]["text"]
                .as_str()
                .unwrap()
                .contains("dirty")
        );
        let repo = s.repo().unwrap();
        let subs = repo.submodules().unwrap();
        let state = |p: &str| subs.iter().find(|s| s.path.to_string() == p).unwrap().state;
        assert_eq!(state("clean"), SubmoduleState::InSync);
        assert_eq!(state("dirty"), SubmoduleState::LocalChanges);
        // No journal entry: the superproject's refs did not move.
        assert!(run(&r, &mut s, "edit.undo", json!({})).is_err());
    }

    #[test]
    fn update_with_init_and_paths() {
        let (r, mut s) = setup(vec![sub("new", None, |_| {}), sub("x", Some(2), |_| {})]);
        let out = run(
            &r,
            &mut s,
            "submodule.update",
            json!({"paths": ["new"], "init": true}),
        )
        .unwrap();
        assert_eq!(out["updated"][0]["path"], "new");
        assert_eq!(out["updated"][0]["from"], Value::Null);
        assert_eq!(out["updated"].as_array().unwrap().len(), 1);
        for bad in [
            json!({"paths": []}),
            json!({"force": true}),
            json!({"paths": ["nope"]}),
        ] {
            assert!(
                run(&r, &mut s, "submodule.update", bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn clone_with_and_without_submodules() {
        let mut remote = MemoryBackend::sample();
        remote.set_submodules(vec![sub("lib", Some(1), |_| {})]);
        let mut opener = MemoryOpener::new();
        opener.insert("remote", remote);
        let mut s = Session::new(Arc::new(opener));
        let r = Registry::builtin();
        for (submodules, state) in [(false, "uninitialized"), (true, "in_sync")] {
            let path = format!("dest-{submodules}");
            run(
                &r,
                &mut s,
                "repo.clone",
                json!({"url": "remote", "path": path, "submodules": submodules}),
            )
            .unwrap();
            let out = run(&r, &mut s, "submodule.list", json!({})).unwrap();
            assert_eq!(out["submodules"][0]["state"], state);
        }
    }
}
