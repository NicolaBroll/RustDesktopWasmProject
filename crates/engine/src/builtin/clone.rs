//! `repo.clone` and `repo.init` (`impl-docs/git/clone.md`): commands without an open repo, like
//! `repo.open`, that create the repo with the [`RepoOpener`](crate::RepoOpener) and open it in the
//! session. Nothing to back up (the destination is absent or empty) and no journal entry (no ref
//! of an open repo moves; the new repo starts with an empty journal).

use super::{Out, RepoOpenResult, opened, to_json};
use crate::commands::parse_params;
use crate::{CloneRequest, EngineError, InitRequest, Session};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Result of `repo.clone`: the repo opened, as in `repo.open`, and git's messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoCloneResult {
    /// `path`, `head` and `commit_graph` of the new repo.
    #[serde(flatten)]
    pub opened: RepoOpenResult,
    /// git's non-progress lines.
    pub messages: Vec<String>,
}

/// Clones `url` into `path` and opens it. Slow: progress and cancellation from [`Session::job`].
///
/// Result: [`RepoCloneResult`].
pub(super) fn repo_clone(s: &mut Session, p: Value) -> Out {
    const ID: &str = "repo.clone";
    let request: CloneRequest = parse_params(ID, p)?;
    request.check()?;
    if s.job().is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    let outcome = s.clone_repo(&request)?;
    let opened = opened(s, request.path)?;
    Ok(to_json(RepoCloneResult {
        opened,
        messages: outcome.messages,
    }))
}

/// `git init` in `path` (initial branch `branch`, otherwise `init.defaultBranch`) and opens it.
///
/// Result: [`RepoOpenResult`], as in `repo.open`.
pub(super) fn repo_init(s: &mut Session, p: Value) -> Out {
    let request: InitRequest = parse_params("repo.init", p)?;
    request.check()?;
    s.init_repo(&request)?;
    Ok(to_json(opened(s, request.path)?))
}

#[cfg(test)]
mod tests {
    use crate::{
        CancelToken, EngineError, GitBackend as _, JobContext, LogQuery, MemoryBackend,
        MemoryOpener, Registry, Session,
    };
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    /// Session without an open repo; the sample repo is the "remote" at `remote`.
    fn setup() -> (Registry, Session, Arc<MemoryOpener>) {
        let mut opener = MemoryOpener::new();
        opener.insert("remote", MemoryBackend::sample());
        opener.insert("empty-remote", MemoryBackend::new());
        opener.occupy("full");
        let opener = Arc::new(opener);
        let s = Session::new(opener.clone());
        (Registry::builtin(), s, opener)
    }

    fn clone(r: &Registry, s: &mut Session, params: Value) -> Result<Value, EngineError> {
        r.run("repo.clone", s, params)
    }

    #[test]
    fn clone_opens_the_new_repo_with_remote_branches() {
        let (r, mut s, opener) = setup();
        let out = clone(&r, &mut s, json!({"url": "remote", "path": "dest"})).unwrap();
        assert_eq!(out["path"], "dest");
        assert_eq!(out["head"]["Branch"]["name"], "main", "{out}");
        assert_eq!(out["messages"], json!([]));
        assert_eq!(s.path(), Some("dest"));
        let repo = s.repo().unwrap();
        let remote = opener.repo("remote").unwrap();
        assert_eq!(
            repo.log(&LogQuery::all()).unwrap(),
            remote.log(&LogQuery::all()).unwrap()
        );
        let refs: Vec<String> = repo.refs().unwrap().into_iter().map(|r| r.name).collect();
        assert!(
            refs.contains(&"refs/remotes/origin/main".to_owned()),
            "{refs:?}"
        );
        assert!(refs.contains(&"refs/heads/main".to_owned()), "{refs:?}");
        assert_eq!(repo.remotes().unwrap(), ["origin"]);
        assert!(repo.upstream("main").unwrap().is_some());
        // The new repo starts with an empty journal.
        let undo = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(undo, EngineError::NotAllowed(_)), "{undo}");
        // Registered: it can be opened again.
        assert!(opener.paths().contains(&"dest".to_owned()));
    }

    #[test]
    fn clone_of_a_branch_and_with_depth() {
        let (r, mut s, _) = setup();
        let remote = MemoryBackend::sample();
        let other = remote
            .refs()
            .unwrap()
            .into_iter()
            .find_map(|r| {
                r.name
                    .strip_prefix("refs/heads/")
                    .filter(|b| *b != "main")
                    .map(str::to_owned)
            })
            .expect("the sample has a second branch");
        let out = clone(
            &r,
            &mut s,
            json!({"url": "remote", "path": "d", "branch": other, "depth": 1}),
        )
        .unwrap();
        assert_eq!(out["head"]["Branch"]["name"], other.as_str(), "{out}");
    }

    #[test]
    fn clone_of_an_empty_remote() {
        let (r, mut s, _) = setup();
        let out = clone(&r, &mut s, json!({"url": "empty-remote", "path": "d"})).unwrap();
        assert_eq!(out["head"]["Branch"]["name"], "main", "{out}");
        assert_eq!(out["messages"].as_array().unwrap().len(), 1, "{out}");
        assert!(s.repo().unwrap().log(&LogQuery::all()).unwrap().is_empty());
    }

    #[test]
    fn clone_refuses_an_occupied_destination_and_keeps_the_open_repo() {
        let (r, mut s, opener) = setup();
        r.run("repo.open", &mut s, json!({"path": "remote"}))
            .unwrap();
        for path in ["full", "remote"] {
            let err = clone(&r, &mut s, json!({"url": "remote", "path": path})).unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{path}: {err}");
        }
        let err = r
            .run("repo.init", &mut s, json!({"path": "full"}))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        assert_eq!(s.path(), Some("remote"));
        assert!(!opener.paths().contains(&"full".to_owned()));
    }

    #[test]
    fn clone_errors() {
        let (r, mut s, _) = setup();
        let mut opener = MemoryOpener::new();
        opener.insert("remote", MemoryBackend::sample());
        opener.deny("secret", "fatal: Authentication failed for 'secret'");
        let opener = Arc::new(opener);
        let mut s2 = Session::new(opener.clone());
        let err = clone(&r, &mut s2, json!({"url": "secret", "path": "d"})).unwrap_err();
        assert!(
            matches!(&err, EngineError::Auth { remote, .. } if remote == "secret"),
            "{err}"
        );
        let err = clone(&r, &mut s2, json!({"url": "nowhere", "path": "d"})).unwrap_err();
        assert!(
            matches!(err, EngineError::RemoteUnreachable { .. }),
            "{err}"
        );
        let err = clone(
            &r,
            &mut s2,
            json!({"url": "remote", "path": "d", "branch": "nope"}),
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        assert!(!opener.paths().contains(&"d".to_owned()));
        assert!(s2.path().is_none());

        for bad in [
            json!({"url": "remote"}),
            json!({"url": "", "path": "d"}),
            json!({"url": "remote", "path": ""}),
            json!({"url": "remote", "path": "d", "depth": 0}),
            json!({"url": "remote", "path": "d", "depth": -1}),
            json!({"url": "remote", "path": "d", "branch": "a..b"}),
            json!({"url": "remote", "path": "d", "other": 1}),
        ] {
            let err = clone(&r, &mut s, bad.clone()).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn cancelled_clone_registers_nothing_and_reports_progress() {
        let (r, mut s, opener) = setup();
        let token = CancelToken::new();
        let seen = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&seen);
        let canceller = token.clone();
        // Cancelled while the transfer reports progress.
        let job = JobContext::new()
            .with_cancel(token)
            .with_progress(move |_| {
                *counter.lock().unwrap() += 1;
                canceller.cancel();
            });
        let err = r
            .run_with(
                "repo.clone",
                &mut s,
                json!({"url": "remote", "path": "d"}),
                job,
            )
            .unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::Cancelled {
                    command: "repo.clone"
                }
            ),
            "{err}"
        );
        assert_eq!(*seen.lock().unwrap(), 1);
        assert!(!opener.paths().contains(&"d".to_owned()));
        assert!(s.path().is_none());
    }

    #[test]
    fn init_with_and_without_branch() {
        let (r, mut s, opener) = setup();
        let out = r.run("repo.init", &mut s, json!({"path": "new"})).unwrap();
        assert_eq!(out["path"], "new");
        assert_eq!(out["head"]["Branch"]["name"], "main", "{out}");
        let out = r
            .run(
                "repo.init",
                &mut s,
                json!({"path": "new2", "branch": "trunk"}),
            )
            .unwrap();
        assert_eq!(out["head"]["Branch"]["name"], "trunk", "{out}");
        assert!(s.repo().unwrap().refs().unwrap().is_empty());
        assert_eq!(s.path(), Some("new2"));
        // Not over an existing repo.
        let err = r
            .run("repo.init", &mut s, json!({"path": "new"}))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        for bad in [
            json!({"path": ""}),
            json!({"path": "x", "branch": "-x"}),
            json!({"path": "x", "branch": "HEAD"}),
            json!({"path": "x", "bare": true}),
        ] {
            let err = r.run("repo.init", &mut s, bad.clone()).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{bad}: {err}"
            );
        }
        assert!(!opener.paths().contains(&"x".to_owned()));
    }

    #[test]
    fn clone_and_init_are_writes() {
        let r = Registry::builtin();
        for id in ["repo.clone", "repo.init"] {
            assert!(r.get(id).unwrap().writes, "{id}");
        }
    }

    /// The typed results read back the JSON of the commands and give the same JSON again: the
    /// UI deserializes into them.
    #[test]
    fn results_round_trip() {
        fn same<T: serde::de::DeserializeOwned + serde::Serialize>(out: Value) -> T {
            let typed: T = serde_json::from_value(out.clone()).unwrap();
            assert_eq!(serde_json::to_value(&typed).unwrap(), out);
            typed
        }
        let (r, mut s, _) = setup();
        let cloned: crate::RepoCloneResult =
            same(clone(&r, &mut s, json!({"url": "remote", "path": "dest"})).unwrap());
        assert_eq!(cloned.opened.path, "dest");
        assert_eq!(cloned.opened.head, s.repo().unwrap().head().unwrap());
        assert!(!cloned.opened.commit_graph);
        assert!(cloned.messages.is_empty());
        // `repo.open` of the same repo gives the same first three fields.
        let reopened: crate::RepoOpenResult =
            same(r.run("repo.open", &mut s, json!({"path": "dest"})).unwrap());
        assert_eq!(reopened, cloned.opened);

        let init: crate::RepoOpenResult = same(
            r.run(
                "repo.init",
                &mut s,
                json!({"path": "new", "branch": "trunk"}),
            )
            .unwrap(),
        );
        assert_eq!(init.path, "new");
        assert_eq!(init.head, s.repo().unwrap().head().unwrap());
    }
}
