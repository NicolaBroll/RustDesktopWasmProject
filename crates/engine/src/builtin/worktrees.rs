//! Linked worktrees (`impl-docs/git/worktrees.md`): `worktree.list` (read), `worktree.add` and
//! `worktree.remove` (writes). Not the commands on the files of the working tree
//! (`worktree.discard`, `worktree.clean`, `worktree.ignore`). Only the branch `worktree.add`
//! creates goes into the journal (as `branch.create`: undo deletes it, refused while the
//! worktree has it checked out); adding or removing a worktree moves no ref.

use super::branch::{find, missing, taken};
use super::{JournalState, NoParams, Out, reread, to_json};
use crate::{
    EngineError, HeadState, JournalEntry, LogQuery, RefMove, RefRestore, Session, WorktreeAdd,
    WorktreeAdded, WorktreeInfo, WorktreeRemove, WorktreeRemoved, WorktreeTarget,
    branch_in_worktree, branch_ref, branch_short, check_branch_name, check_new_branch_name,
    check_worktree_path,
};
use project_domain::Oid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters of `worktree.add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorktreeAddParams {
    /// The new worktree: an absent or empty folder, absolute or relative to the root of the
    /// open worktree.
    pub path: String,
    /// An existing local branch (short name) to check out there; it must not be checked out in
    /// any worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// A new local branch (short name) created at `start` and checked out there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_branch: Option<String>,
    /// Where `new_branch`, or the detached HEAD without a branch, starts (rev); without: HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
}

/// Result of `worktree.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeListResult {
    /// The main worktree first, then the linked ones.
    pub worktrees: Vec<WorktreeInfo>,
}

/// Result of `worktree.add`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorktreeAddResult {
    /// `path` (absolute: what `repo.open` takes), `messages`, `warnings`.
    #[serde(flatten)]
    pub added: WorktreeAdded,
    /// The branch checked out there (full name); `null`: detached HEAD.
    pub branch: Option<String>,
    /// `branch` was created by the command (`new_branch`).
    pub created_branch: bool,
    /// The commit checked out.
    pub head: Oid,
    /// The worktrees re-read; `null` if they could not be (see `worktrees_error`).
    pub worktrees: Option<Vec<WorktreeInfo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktrees_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Result of `worktree.remove`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorktreeRemoveResult {
    /// `path`, `backup`, `messages`.
    #[serde(flatten)]
    pub removed: WorktreeRemoved,
    /// The worktrees re-read; `null` if they could not be (see `worktrees_error`).
    pub worktrees: Option<Vec<WorktreeInfo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktrees_error: Option<String>,
}

/// The worktrees of the repository.
///
/// Result: [`WorktreeListResult`].
pub(super) fn worktree_list(s: &mut Session, NoParams {}: NoParams) -> Out {
    Ok(to_json(WorktreeListResult {
        worktrees: s.repo()?.worktrees()?,
    }))
}

/// A new worktree on an existing branch, on a new branch or on a detached commit.
///
/// Result: [`WorktreeAddResult`].
pub(super) fn worktree_add(s: &mut Session, params: WorktreeAddParams) -> Out {
    const ID: &str = "worktree.add";
    let invalid = |reason| EngineError::InvalidParams { id: ID, reason };
    let WorktreeAddParams {
        path,
        branch,
        new_branch,
        start,
    } = params;
    if branch.is_some() && new_branch.is_some() {
        return Err(invalid(msg!("engine-worktree-branch-and-new")));
    }
    if branch.is_some() && start.is_some() {
        return Err(invalid(msg!("engine-worktree-branch-and-start")));
    }
    if let Some(name) = &branch {
        check_branch_name(ID, name)?;
    }
    if let Some(name) = &new_branch {
        check_new_branch_name(ID, name)?;
    }
    let request = |target| WorktreeAdd {
        path: path.clone(),
        target,
    };
    check_worktree_path(&path)?;

    let (repo, journal) = s.repo_and_journal()?;
    let refs = repo.refs()?;
    // The branch must be free in every worktree (git refuses it too, with `--force` aside).
    let in_use = |full: &str| -> Result<(), EngineError> {
        match repo
            .worktrees()?
            .into_iter()
            .find(|w| w.branch.as_deref() == Some(full))
        {
            Some(w) => Err(branch_in_worktree(branch_short(full), &w.path)),
            None => Ok(()),
        }
    };
    let start_commit = |rev: &Option<String>| -> Result<Oid, EngineError> {
        match rev {
            Some(rev) => repo
                .log(&LogQuery::page(0, 1).from_rev(rev.clone()))?
                .first()
                .map(|c| c.id)
                .ok_or_else(|| EngineError::UnknownRev(rev.clone())),
            None => repo
                .head()?
                .target()
                .ok_or_else(|| EngineError::NotAllowed(msg!("engine-branch-unborn-start"))),
        }
    };

    if let Some(name) = branch {
        let full = branch_ref(&name);
        let head = find(&refs, &full).ok_or_else(|| missing(&name))?.target;
        in_use(&full)?;
        let added = repo.add_worktree(&request(WorktreeTarget::Branch(name)))?;
        let (worktrees, worktrees_error) = reread(repo.worktrees());
        return Ok(to_json(WorktreeAddResult {
            added,
            branch: Some(full),
            created_branch: false,
            head,
            worktrees,
            worktrees_error,
            journal: JournalState::of(journal),
        }));
    }

    let target = start_commit(&start)?;
    let Some(name) = new_branch else {
        let added = repo.add_worktree(&request(WorktreeTarget::Detached(target)))?;
        let (worktrees, worktrees_error) = reread(repo.worktrees());
        return Ok(to_json(WorktreeAddResult {
            added,
            branch: None,
            created_branch: false,
            head: target,
            worktrees,
            worktrees_error,
            journal: JournalState::of(journal),
        }));
    };

    let full = branch_ref(&name);
    if find(&refs, &full).is_some() {
        return Err(taken(&name));
    }
    let head = HeadState::from_head(&repo.head()?);
    repo.create_branch(&name, &target)?;
    let added = match repo.add_worktree(&request(WorktreeTarget::Branch(name.clone()))) {
        Ok(added) => added,
        Err(error) => {
            // The branch was made for the worktree only: deleted again if still where it was.
            let rollback = RefRestore {
                command: ID,
                head_expected: head.clone(),
                head_target: head,
                refs: vec![RefMove {
                    name: full.clone(),
                    expected: Some(target),
                    target: None,
                }],
                reflog: format!("project: worktree.add {full} rolled back"),
            };
            // Rolled back: nothing changed, git's (or the destination's) error as it is.
            return Err(match repo.restore_refs(&rollback) {
                Ok(_) => error,
                Err(e) => EngineError::GitWithReason {
                    command: "worktree".to_owned(),
                    reason: msg!(
                        "engine-worktree-branch-not-rolled-back",
                        name = name.escape_debug(),
                        error = error,
                        rollback = e
                    ),
                },
            });
        }
    };
    journal.record(JournalEntry::branch_create(&full, target, head));
    let (worktrees, worktrees_error) = reread(repo.worktrees());
    Ok(to_json(WorktreeAddResult {
        added,
        branch: Some(full),
        created_branch: true,
        head: target,
        worktrees,
        worktrees_error,
        journal: JournalState::of(journal),
    }))
}

/// Removes a linked worktree without changes, untracked or ignored files, with `confirm`.
///
/// Result: [`WorktreeRemoveResult`].
pub(super) fn worktree_remove(s: &mut Session, request: WorktreeRemove) -> Out {
    if request.path.is_empty() {
        return Err(EngineError::InvalidParams {
            id: "worktree.remove",
            reason: msg!("engine-clone-path-empty"),
        });
    }
    let repo = s.repo_mut()?;
    let removed = repo.remove_worktree(&request)?;
    let (worktrees, worktrees_error) = reread(repo.worktrees());
    Ok(to_json(WorktreeRemoveResult {
        removed,
        worktrees,
        worktrees_error,
    }))
}

#[cfg(test)]
mod tests {
    use crate::{
        EngineError, MemoryBackend, MemoryOpener, MemoryWorktree, Registry, Session,
        WorktreeContents, WorktreeInfo,
    };
    use project_domain::Operation;
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn linked(path: &str, f: impl FnOnce(&mut MemoryWorktree)) -> MemoryWorktree {
        let mut w = MemoryWorktree {
            info: WorktreeInfo {
                path: path.into(),
                main: false,
                current: false,
                bare: false,
                head: Some(MemoryBackend::fake_oid(77)),
                branch: None,
                detached: true,
                locked: false,
                lock_reason: None,
                prunable: false,
            },
            contents: Some(WorktreeContents::default()),
            unreferenced: false,
        };
        f(&mut w);
        w
    }

    fn setup(worktrees: Vec<MemoryWorktree>) -> (Registry, Session) {
        let mut repo = MemoryBackend::sample();
        for w in worktrees {
            repo.add_linked_worktree(w);
        }
        repo.occupy_path("/full");
        let mut opener = MemoryOpener::new();
        opener.insert("repo", repo);
        let mut s = Session::new(Arc::new(opener));
        s.open("repo").unwrap();
        (Registry::builtin(), s)
    }

    fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Result<Value, EngineError> {
        r.run(id, s, params)
    }

    fn paths(out: &Value) -> Vec<String> {
        out["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["path"].as_str().unwrap().to_owned())
            .collect()
    }

    fn not_allowed(err: EngineError, key: &str) {
        match &err {
            EngineError::NotAllowed(m) => assert_eq!(m.key(), key, "{err}"),
            _ => panic!("expected NotAllowed {key}: {err}"),
        }
    }

    fn other_branch(s: &Session) -> String {
        s.repo()
            .unwrap()
            .refs()
            .unwrap()
            .into_iter()
            .find_map(|r| {
                r.name
                    .strip_prefix("refs/heads/")
                    .filter(|b| *b != "main")
                    .map(str::to_owned)
            })
            .unwrap()
    }

    #[test]
    fn list_has_the_main_worktree_first() {
        let (r, mut s) = setup(vec![linked("/w", |_| {})]);
        let out = run(&r, &mut s, "worktree.list", json!({})).unwrap();
        assert_eq!(paths(&out), ["/repo", "/w"]);
        assert_eq!(out["worktrees"][0]["main"], true);
        assert_eq!(out["worktrees"][0]["current"], true);
        assert!(run(&r, &mut s, "worktree.list", json!({"x": 1})).is_err());
    }

    #[test]
    fn add_on_an_existing_branch_and_detached() {
        let (r, mut s) = setup(vec![]);
        let branch = other_branch(&s);
        let out = run(
            &r,
            &mut s,
            "worktree.add",
            json!({"path": "/a", "branch": branch}),
        )
        .unwrap();
        assert_eq!(out["path"], "/a");
        assert_eq!(out["branch"], format!("refs/heads/{branch}"));
        assert_eq!(out["created_branch"], false);
        assert_eq!(out["can_undo"], false, "nothing in the journal");
        // Already checked out there, and `main` here: refused.
        for (path, b) in [("/b", branch.as_str()), ("/c", "main")] {
            let err = run(
                &r,
                &mut s,
                "worktree.add",
                json!({"path": path, "branch": b}),
            )
            .unwrap_err();
            not_allowed(err, "engine-worktree-branch-in-use");
        }
        let out = run(
            &r,
            &mut s,
            "worktree.add",
            json!({"path": "d", "start": "main"}),
        )
        .unwrap();
        assert_eq!(out["path"], "/repo/d");
        assert_eq!(out["branch"], Value::Null);
        assert_eq!(paths(&out), ["/repo", "/a", "/repo/d"]);
        assert_eq!(out["worktrees"][2]["detached"], true);
    }

    #[test]
    fn add_refuses_bad_parameters_and_occupied_paths() {
        let (r, mut s) = setup(vec![linked("/w", |_| {})]);
        for params in [
            json!({"path": "/a", "branch": "main", "new_branch": "x"}),
            json!({"path": "/a", "branch": "main", "start": "main"}),
            json!({"path": "", "new_branch": "x"}),
            json!({"path": "a\nb"}),
            json!({"path": "/a", "new_branch": "-x"}),
            json!({"path": "/a", "force": true}),
        ] {
            let err = run(&r, &mut s, "worktree.add", params.clone()).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{params}: {err}"
            );
        }
        let err = run(
            &r,
            &mut s,
            "worktree.add",
            json!({"path": "/a", "branch": "nope"}),
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        let err = run(
            &r,
            &mut s,
            "worktree.add",
            json!({"path": "/a", "start": "nope"}),
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
        for path in ["/full", "/w", "/repo"] {
            let err = run(&r, &mut s, "worktree.add", json!({"path": path})).unwrap_err();
            not_allowed(err, "engine-clone-not-empty");
        }
    }

    #[test]
    fn new_branch_goes_into_the_journal_and_is_rolled_back_on_failure() {
        let (r, mut s) = setup(vec![]);
        // The path is taken: the branch just created is deleted again.
        let err = run(
            &r,
            &mut s,
            "worktree.add",
            json!({"path": "/full", "new_branch": "topic"}),
        )
        .unwrap_err();
        not_allowed(err, "engine-clone-not-empty");
        let has = |s: &Session| {
            s.repo()
                .unwrap()
                .refs()
                .unwrap()
                .iter()
                .any(|r| r.name == "refs/heads/topic")
        };
        assert!(!has(&s));
        let err = run(&r, &mut s, "edit.undo", json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

        let out = run(
            &r,
            &mut s,
            "worktree.add",
            json!({"path": "/t", "new_branch": "topic"}),
        )
        .unwrap();
        assert_eq!(out["created_branch"], true);
        assert_eq!(out["next_undo"]["key"], "engine-journal-branch-create");
        assert!(has(&s));
        // The branch is open in the worktree: neither undo nor branch.delete removes it.
        let err = run(&r, &mut s, "edit.undo", json!({})).unwrap_err();
        not_allowed(err, "engine-undo-delete-worktree-branch");
        let err = run(
            &r,
            &mut s,
            "branch.delete",
            json!({"name": "topic", "confirm": true}),
        )
        .unwrap_err();
        not_allowed(err, "engine-branch-delete-other-worktree");
        assert!(has(&s));
        // Once the worktree is removed, undo deletes it.
        run(
            &r,
            &mut s,
            "worktree.remove",
            json!({"path": "/t", "confirm": true}),
        )
        .unwrap();
        run(&r, &mut s, "edit.undo", json!({})).unwrap();
        assert!(!has(&s));
    }

    #[test]
    fn remove_asks_for_confirm_and_saves_an_unreferenced_head() {
        let (r, mut s) = setup(vec![
            linked("/plain", |_| {}),
            linked("/lost", |w| w.unreferenced = true),
        ]);
        let err = run(&r, &mut s, "worktree.remove", json!({"path": "/plain"})).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConfirmRequired { reason, .. } if reason.key() == "engine-worktree-remove-confirm"),
            "{err:?}"
        );
        let err = run(&r, &mut s, "worktree.remove", json!({"path": "/lost"})).unwrap_err();
        assert!(
            err.to_string().contains("refs/project/backup/worktree-"),
            "{err}"
        );
        assert_eq!(s.repo().unwrap().worktrees().unwrap().len(), 3);

        let out = run(
            &r,
            &mut s,
            "worktree.remove",
            json!({"path": "/plain", "confirm": true}),
        )
        .unwrap();
        assert_eq!(out["backup"], Value::Null);
        let out = run(
            &r,
            &mut s,
            "worktree.remove",
            json!({"path": "/lost", "confirm": true}),
        )
        .unwrap();
        let backup = out["backup"].as_str().unwrap();
        assert!(backup.starts_with("refs/project/backup/worktree-"), "{out}");
        assert_eq!(paths(&out), ["/repo"]);
        let err = run(
            &r,
            &mut s,
            "worktree.remove",
            json!({"path": "/lost", "confirm": true}),
        )
        .unwrap_err();
        not_allowed(err, "engine-worktree-unknown");
    }

    #[test]
    fn remove_refusals() {
        let contents = |f: fn(&mut WorktreeContents)| {
            let mut c = WorktreeContents::default();
            f(&mut c);
            Some(c)
        };
        let (r, mut s) = setup(vec![
            linked("/locked", |w| {
                w.info.locked = true;
                w.info.lock_reason = Some("usb disk".into());
            }),
            linked("/op", |w| {
                w.contents = contents(|c| c.operation = Some(Operation::Rebase))
            }),
            linked("/changes", |w| {
                w.contents = contents(|c| c.changes = vec!["a".into()])
            }),
            linked("/untracked", |w| {
                w.contents = contents(|c| c.untracked = vec!["n".into()])
            }),
            linked("/ignored", |w| {
                w.contents = contents(|c| c.ignored = vec!["t/".into()])
            }),
            linked("/hidden", |w| {
                w.contents = contents(|c| c.hidden = vec!["h".into()])
            }),
            linked("/gone", |w| w.contents = None),
        ]);
        for (path, key) in [
            ("/repo", "engine-worktree-main"),
            ("/locked", "engine-worktree-locked-reason"),
            ("/op", "engine-worktree-operation"),
            ("/changes", "engine-worktree-changes"),
            ("/untracked", "engine-worktree-untracked"),
            ("/ignored", "engine-worktree-ignored"),
            ("/hidden", "engine-worktree-hidden"),
            ("/nowhere", "engine-worktree-unknown"),
        ] {
            let err = run(
                &r,
                &mut s,
                "worktree.remove",
                json!({"path": path, "confirm": true}),
            )
            .unwrap_err();
            not_allowed(err, key);
        }
        let list = run(&r, &mut s, "worktree.list", json!({})).unwrap();
        assert_eq!(list["worktrees"][7]["prunable"], true, "{list}");
        // A worktree whose folder is gone is removed (only its entry).
        run(
            &r,
            &mut s,
            "worktree.remove",
            json!({"path": "/gone", "confirm": true}),
        )
        .unwrap();
        let err = run(
            &r,
            &mut s,
            "worktree.remove",
            json!({"path": "", "confirm": true}),
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    }
}
