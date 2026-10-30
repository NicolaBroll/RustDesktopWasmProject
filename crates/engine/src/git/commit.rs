//! Commit (`commit.create`): parameters, result and checks shared by the backends
//! (`impl-docs/git/commit.md`).
//!
//! The backends call the same checks before writing, so `MemoryBackend` and
//! `CliBackend` reject the same cases with the same errors: empty message, conflicts
//! in the index, rebase/cherry-pick/revert in progress, nothing to commit, `amend` without a commit
//! or during a merge, `amend` of an already published commit without `"confirm": true`.

use crate::EngineError;
use project_domain::{Commit, EntryKind, Oid, Operation, Status};
use project_domain::{HEADS_PREFIX, REMOTES_PREFIX, TAGS_PREFIX};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Id of the command, used in parameter errors.
const COMMAND: &str = "commit.create";

/// Parameters of `commit.create`: `{"message": string, "amend"?: bool, "confirm"?: bool}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommitRequest {
    /// The message, passed to git as is (via stdin, `git commit -F -`). Cleanup is done by git
    /// according to the user's `commit.cleanup`: with no configuration, with `-F` it is `whitespace`
    /// (trailing spaces on lines, leading, trailing and repeated blank lines removed; lines
    /// starting with `#` **stay**, e.g. `#123 fixes ...`).
    pub message: String,
    /// Rewrites the last commit (`git commit --amend`) instead of creating a new one: same
    /// author and same parents, tree from the index, new message.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub amend: bool,
    /// Needed only with `amend` of an already published commit (reachable from a remote
    /// branch): without it, [`EngineError::ConfirmRequired`]. Ignored elsewhere.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
    /// Not a parameter of `commit.create`: set by `cherry_pick.continue` /
    /// `revert.continue` ([`CommitRequest::concluding`]) to conclude the cherry-pick or the
    /// revert in progress with the commit (with `CHERRY_PICK_HEAD` git keeps the original author).
    /// `None`: a cherry-pick or revert in progress rejects the commit.
    #[serde(skip)]
    pub conclude: Option<Operation>,
}

impl CommitRequest {
    /// A new commit with `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            amend: false,
            confirm: false,
            conclude: None,
        }
    }

    /// The same, to conclude the operation `op` in progress (cherry-pick or revert, from
    /// `*.continue`): [`check_commit_state`] lets it through.
    pub fn concluding(mut self, op: Operation) -> Self {
        self.conclude = Some(op);
        self
    }

    /// The same, as `amend` of the last commit.
    pub fn amend(mut self) -> Self {
        self.amend = true;
        self
    }

    /// The same, with `"confirm": true`.
    pub fn confirmed(mut self) -> Self {
        self.confirm = true;
        self
    }

    /// [`EngineError::InvalidParams`] for an empty or whitespace-only message (git would
    /// reject it anyway: no `--allow-empty-message`) or one with a NUL byte (git does not
    /// accept it in a message). A comment-only message passes: with the `whitespace` cleanup
    /// (the default with `-F`) it is a valid message; if the user's `commit.cleanup`
    /// strips comments and nothing is left, git rejects the commit and the backend gives the same
    /// error ([`EngineError::empty_commit_message`]).
    pub fn check_message(&self) -> Result<(), EngineError> {
        let reason = if self.message.trim().is_empty() {
            msg!("engine-commit-message-empty")
        } else if self.message.contains('\0') {
            msg!("engine-commit-message-nul")
        } else {
            return Ok(());
        };
        Err(EngineError::InvalidParams {
            id: COMMAND,
            reason,
        })
    }
}

/// Result of [`GitIndex::commit`](crate::GitIndex::commit). `commit.create` adds the
/// state (`status`).
///
/// It has the before and after values needed by the operation journal (§7): the moved ref
/// (`branch`, or detached HEAD), its value before (`previous`) and after (`commit.id`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitOutcome {
    /// The created commit, as in `repo.log`.
    pub commit: Commit,
    /// HEAD before the commit; `None` for the first commit of a branch. With `amend`, the
    /// rewritten commit.
    pub previous: Option<Oid>,
    /// The moved branch, full name (e.g. `refs/heads/main`); `None` with detached HEAD.
    pub branch: Option<String>,
    /// Only with `amend`: the backup ref (`refs/project/backup/amend-...`) pointing at the
    /// rewritten commit, saved before writing. `None` also if the `amend` produced the same
    /// commit (`commit.id == previous`: nothing changed and the backup was removed).
    pub backup: Option<String>,
    /// Only with `amend`: the other local branches and tags (full names) that contain the
    /// rewritten commit and therefore, after the `amend`, stay on the old history (they diverge
    /// from HEAD). Empty if none. Field added after the first version: whoever reads it from an
    /// older JSON finds it empty.
    #[serde(default)]
    pub also_on: Vec<String>,
}

/// How the refs containing the commit to rewrite with `amend` are split (`containing`,
/// full names from `refs/heads/`, `refs/remotes/`, `refs/tags/`; `branch` is the current one):
/// `(published, also_on)`.
///
/// - Published (they ask for `"confirm": true`, [`check_amend_published`]): remote branches and
///   tags. A tag would stay on the old commit. A push without a tracking ref (push by
///   URL, or deleted remote refs) is not seen: the commit counts as unpublished.
/// - Also on (`also_on` of the result): the other local branches and tags, which after the `amend`
///   diverge from HEAD.
pub fn amend_impact(containing: &[String], branch: Option<&str>) -> (Vec<String>, Vec<String>) {
    let published = containing
        .iter()
        .filter(|n| n.starts_with(REMOTES_PREFIX) || n.starts_with(TAGS_PREFIX))
        .cloned()
        .collect();
    let also_on = containing
        .iter()
        .filter(|n| {
            (n.starts_with(HEADS_PREFIX) && Some(n.as_str()) != branch)
                || n.starts_with(TAGS_PREFIX)
        })
        .cloned()
        .collect();
    (published, also_on)
}

/// The state checks before a commit, the same for all backends. `has_head` says whether
/// HEAD already has a commit. Nothing has been modified when they fail.
///
/// - Conflicted files in the index → [`EngineError::NotAllowed`] (git would reject it anyway).
/// - Rebase, cherry-pick, revert or `git am` in progress → [`EngineError::NotAllowed`]: `git commit`
///   would conclude or change the operation in hard-to-see ways (a cherry-pick would take
///   the original commit's author and close the operation; during a rebase the commit
///   would end up in the sequence). Cherry-pick and revert conclude with `*.continue` ([`CommitRequest::concluding`]); the others from the
///   terminal.
/// - Merge in progress: the commit is the **merge commit** (git adds `MERGE_HEAD` to the parents),
///   even with the index equal to HEAD (merge without changes, e.g. `-s ours`). With `amend` it is
///   rejected, as git does.
/// - Bisect in progress: nothing special, the commit is made.
/// - `amend` without a commit in HEAD → [`EngineError::NotAllowed`].
/// - Without `amend`, nothing staged (and no merge in progress) → [`EngineError::NothingToCommit`]:
///   never an empty commit. An `intent-to-add` file (`git add -N`) does not count, as for git.
pub fn check_commit_state(
    status: &Status,
    has_head: bool,
    request: &CommitRequest,
) -> Result<(), EngineError> {
    let amend = request.amend;
    let conflicts: Vec<String> = status
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
        .map(|e| format!("`{}`", e.path))
        .collect();
    if !conflicts.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-commit-conflicts",
            files = conflicts.join(", ")
        )));
    }
    match status.operation {
        Some(
            op @ (Operation::Rebase | Operation::CherryPick | Operation::Revert | Operation::Am),
        ) if !(request.conclude == Some(op)
            && matches!(op, Operation::CherryPick | Operation::Revert)
            && !amend) =>
        {
            // The selector of `engine-commit-operation`.
            let operation = match op {
                Operation::Rebase => "rebase",
                Operation::CherryPick => "cherry-pick",
                Operation::Am => "am",
                _ => "revert",
            };
            return Err(EngineError::NotAllowed(msg!(
                "engine-commit-operation",
                operation = operation
            )));
        }
        Some(Operation::Merge) if amend => {
            return Err(EngineError::NotAllowed(msg!("engine-commit-amend-merge")));
        }
        _ => {}
    }
    if amend && !has_head {
        return Err(EngineError::NotAllowed(msg!("engine-commit-amend-unborn")));
    }
    let merging = status.operation == Some(Operation::Merge);
    if !amend && !merging && !status.entries.iter().any(|e| e.is_staged()) {
        return Err(EngineError::NothingToCommit);
    }
    Ok(())
}

/// With `amend` of an already published commit (`published`: the remote branches and tags it is
/// reachable from, see [`amend_impact`]) `"confirm": true` is needed: whoever already downloaded the commit
/// would find a different history, the next push would have to be forced and a tag would stay on the
/// old commit. Without confirmation, [`EngineError::ConfirmRequired`] with the refs.
pub fn check_amend_published(
    request: &CommitRequest,
    published: &[String],
) -> Result<(), EngineError> {
    if !request.amend || request.confirm || published.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = published.iter().map(|n| format!("`{n}`")).collect();
    Err(EngineError::ConfirmRequired {
        command: COMMAND,
        reason: msg!("engine-commit-amend-published", refs = names.join(", ")),
        expected: None,
    })
}

/// The message as git cleans it with `--cleanup=whitespace` (the default of `git commit -F`):
/// trailing spaces on lines removed, leading and trailing blank lines removed, consecutive blank lines
/// reduced to one, a final newline. Comment lines stay. Used by the
/// `MemoryBackend`, which has no git; the `CliBackend` leaves it to git.
#[cfg(any(test, feature = "test-support"))]
pub fn cleanup_message(message: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in message.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push('\n');
            blank = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// `(subject, body)` of a cleaned message, like git's `%s` and `%b`: the subject is the first
/// paragraph with its lines joined by a space, the body the rest after the blank lines, without
/// trailing newlines.
#[cfg(any(test, feature = "test-support"))]
pub fn split_message(message: &str) -> (String, String) {
    let mut lines = message.lines();
    let mut summary = Vec::new();
    for line in lines.by_ref() {
        if line.trim().is_empty() {
            if summary.is_empty() {
                continue;
            }
            break;
        }
        summary.push(line.trim());
    }
    let rest: Vec<&str> = lines.skip_while(|l| l.trim().is_empty()).collect();
    let body = rest.join("\n").trim_end_matches('\n').to_owned();
    (summary.join(" "), body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitRead, MemoryBackend};
    use project_domain::{Change, RepoPath, StatusEntry};

    #[test]
    fn message_checks() {
        for bad in ["", "  \n\t\n", "a\0b"] {
            let err = CommitRequest::new(bad).check_message().unwrap_err();
            assert!(matches!(err, EngineError::InvalidParams { .. }), "{bad:?}");
        }
        for good in ["x", "# just a comment", "#123 fixes\n"] {
            CommitRequest::new(good).check_message().unwrap();
        }
    }

    #[test]
    fn cleanup_like_git_whitespace() {
        // `printf '...' | git commit -F -` with `--cleanup=whitespace`.
        assert_eq!(
            cleanup_message("\n# comment\n\nTitle  \n\n\nbody\n# other\n\n"),
            "# comment\n\nTitle\n\nbody\n# other\n"
        );
        assert_eq!(cleanup_message("only"), "only\n");
        assert_eq!(cleanup_message("  \n"), "");
    }

    #[test]
    fn split_like_git_format() {
        assert_eq!(
            split_message("Title\non two lines\n\n\nBody\n\nother\n"),
            ("Title on two lines".to_owned(), "Body\n\nother".to_owned())
        );
        assert_eq!(split_message("only\n"), ("only".to_owned(), String::new()));
    }

    fn status_with(entries: Vec<StatusEntry>, operation: Option<Operation>) -> Status {
        let mut status = MemoryBackend::sample().status().unwrap();
        status.entries = entries;
        status.operation = operation;
        status
    }

    #[test]
    fn state_checks() {
        let sample = MemoryBackend::sample().status().unwrap();
        check_commit_state(&sample, true, &CommitRequest::new("m")).unwrap();
        check_commit_state(&sample, true, &CommitRequest::new("m").amend()).unwrap();
        // Only unstaged changes and untracked files: nothing to commit.
        let unstaged: Vec<_> = sample
            .entries
            .iter()
            .filter(|e| !e.is_staged())
            .cloned()
            .collect();
        let status = status_with(unstaged.clone(), None);
        assert!(matches!(
            check_commit_state(&status, true, &CommitRequest::new("m")),
            Err(EngineError::NothingToCommit)
        ));
        // But with `amend` only the message changes.
        check_commit_state(&status, true, &CommitRequest::new("m").amend()).unwrap();
        assert!(matches!(
            check_commit_state(&status, false, &CommitRequest::new("m").amend()),
            Err(EngineError::NotAllowed(_))
        ));
        // A merge without changes concludes; no `amend`.
        let merging = status_with(unstaged, Some(Operation::Merge));
        check_commit_state(&merging, true, &CommitRequest::new("m")).unwrap();
        assert!(check_commit_state(&merging, true, &CommitRequest::new("m").amend()).is_err());
        for op in [
            Operation::Rebase,
            Operation::CherryPick,
            Operation::Revert,
            Operation::Am,
        ] {
            let status = status_with(sample.entries.clone(), Some(op));
            let err = check_commit_state(&status, true, &CommitRequest::new("m")).unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{op:?}");
            // Only `*.continue` concludes cherry-pick and revert (never with `amend`).
            let concluding = CommitRequest::new("m").concluding(op);
            let ok = check_commit_state(&status, true, &concluding).is_ok();
            assert_eq!(
                ok,
                matches!(op, Operation::CherryPick | Operation::Revert),
                "{op:?}"
            );
            assert!(check_commit_state(&status, true, &concluding.amend()).is_err());
        }
        let status = status_with(sample.entries.clone(), Some(Operation::Bisect));
        check_commit_state(&status, true, &CommitRequest::new("m")).unwrap();
        // A conflict blocks everything.
        let mut entries = sample.entries;
        entries.push(StatusEntry {
            path: RepoPath::from("c.txt"),
            index: Change::Unmerged,
            worktree: Change::Unmerged,
            kind: EntryKind::Unmerged(project_domain::UnmergedInfo {
                conflict: project_domain::Conflict::BothModified,
                submodule: None,
                mode_base: 0o100644,
                mode_ours: 0o100644,
                mode_theirs: 0o100644,
                mode_worktree: 0o100644,
                oid_base: Oid::from_bytes([1; Oid::LEN]),
                oid_ours: Oid::from_bytes([2; Oid::LEN]),
                oid_theirs: Oid::from_bytes([3; Oid::LEN]),
            }),
        });
        let err = check_commit_state(
            &status_with(entries, Some(Operation::Merge)),
            true,
            &CommitRequest::new("m"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("`c.txt`"), "{err}");
    }

    #[test]
    fn amend_impact_splits_published_and_diverging_refs() {
        let refs: Vec<String> = [
            "refs/heads/another",
            "refs/heads/main",
            "refs/remotes/origin/main",
            "refs/tags/v1",
        ]
        .map(str::to_owned)
        .into();
        let (published, also_on) = amend_impact(&refs, Some("refs/heads/main"));
        assert_eq!(published, ["refs/remotes/origin/main", "refs/tags/v1"]);
        assert_eq!(also_on, ["refs/heads/another", "refs/tags/v1"]);
        // With detached HEAD `main` diverges too.
        let (_, also_on) = amend_impact(&refs, None);
        assert_eq!(
            also_on,
            ["refs/heads/another", "refs/heads/main", "refs/tags/v1"]
        );
    }

    #[test]
    fn amend_of_published_commit_needs_confirm() {
        let published = vec!["refs/remotes/origin/main".to_owned()];
        let amend = CommitRequest::new("x").amend();
        let err = check_amend_published(&amend, &published).unwrap_err();
        assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
        assert!(err.to_string().contains("origin/main"), "{err}");
        assert!(err.to_string().contains("\"confirm\": true"), "{err}");
        check_amend_published(&amend.clone().confirmed(), &published).unwrap();
        check_amend_published(&amend, &[]).unwrap();
        check_amend_published(&CommitRequest::new("x"), &published).unwrap();
    }
}
