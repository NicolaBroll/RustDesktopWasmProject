//! Controlled moving of refs for `edit.undo` and `edit.redo`
//! (`impl-docs/ARCHITECTURE-git.md` §7).
//!
//! The operation journal ([`Journal`](crate::Journal)) remembers, for every command that
//! moved refs, HEAD and the refs before and after. Undoing means bringing them back to the "before" values,
//! redoing to the "after" values, and **only** if in the meantime nobody touched them: a
//! [`RefRestore`] tells the backends which values they expect now (`expected`) and which to write
//! (`target`). The checks shared by the backends are here, so `MemoryBackend` and `CliBackend`
//! reject the same cases with the same errors.

use crate::{EngineError, Message, MessageArg, ToArg};
use project_domain::{Oid, Operation};

use serde::{Deserialize, Serialize};

/// Where HEAD points at a given moment, for the journal: on a branch (full name, e.g.
/// `refs/heads/main`, even if the branch has no commits yet) or detached on a commit.
///
/// JSON: `{"branch": "refs/heads/main"}` or `{"detached": "<oid>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadState {
    Branch(String),
    Detached(Oid),
}

impl HeadState {
    /// HEAD as read from [`GitBackend::head`](crate::GitBackend::head): the branch with its full
    /// name (`refs/heads/<name>`), or detached.
    pub fn from_head(head: &project_domain::Head) -> Self {
        match head {
            project_domain::Head::Branch { name, .. } if name.starts_with("refs/") => {
                HeadState::Branch(name.clone())
            }
            project_domain::Head::Branch { name, .. } => {
                HeadState::Branch(format!("refs/heads/{name}"))
            }
            project_domain::Head::Detached(oid) => HeadState::Detached(*oid),
        }
    }
}

impl HeadState {
    /// The position of HEAD in the text of an error: "on branch `x`", "detached at abc1234".
    pub fn message(&self) -> Message {
        match self {
            HeadState::Branch(name) => msg!("engine-head-branch", name = name),
            HeadState::Detached(oid) => msg!("engine-head-detached", id = short(oid)),
        }
    }
}

impl ToArg for HeadState {
    fn to_arg(&self) -> MessageArg {
        self.message().to_arg()
    }
}

impl std::fmt::Display for HeadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeadState::Branch(name) => write!(f, "on branch `{name}`"),
            HeadState::Detached(oid) => write!(f, "detached at {}", short(oid)),
        }
    }
}

/// A ref to move from `expected` (the value it must have now; `None` = must not exist)
/// to `target` (`None` = to delete).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefMove {
    /// Full name, under `refs/` (e.g. `refs/heads/main`).
    pub name: String,
    pub expected: Option<Oid>,
    pub target: Option<Oid>,
}

/// What `edit.undo`/`edit.redo` ask of the backend ([`GitBackend::restore_refs`]): HEAD and the
/// refs must now equal `expected`, and must be moved to `target` all together or none.
///
/// [`GitBackend::restore_refs`]: crate::GitBackend::restore_refs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRestore {
    /// `edit.undo` or `edit.redo`: in errors and in the name of the backup refs.
    pub command: &'static str,
    /// HEAD as it must be now.
    pub head_expected: HeadState,
    /// HEAD as it must become: only the same branch as `head_expected` (HEAD is not touched)
    /// or another commit with detached HEAD. A branch change does not go through here: undo and redo
    /// of `branch.checkout` and `branch.rename` use
    /// [`GitBackend::checkout`](crate::GitBackend::checkout) and
    /// [`GitBackend::rename_branch`](crate::GitBackend::rename_branch), which also update the
    /// worktree, reflog and configuration.
    pub head_target: HeadState,
    pub refs: Vec<RefMove>,
    /// Reflog message (`update-ref -m`), e.g. `project: undo Commit a0744b4 «...»`.
    pub reflog: String,
}

impl RefRestore {
    /// The backup kind in the ref name (`refs/project/backup/undo-...`): `undo` or `redo`.
    /// `remote` for `remote.remove` (the commits that stayed only in the removed remote branches).
    pub fn backup_kind(&self) -> &'static str {
        match self.command {
            "remote.remove" => "remote",
            "branch.reset" => "reset",
            command => command.strip_prefix("edit.").unwrap_or("undo"),
        }
    }

    /// The checks before touching anything, the same for all backends (`operation` is
    /// the operation in progress in the repo, as in [`Status::operation`]); nothing has changed when
    /// they fail.
    ///
    /// [`Status::operation`]: project_domain::Status::operation
    ///
    /// - Merge, rebase, cherry-pick, revert, bisect or `git am` in progress → [`EngineError::NotAllowed`]:
    ///   moving a branch in the middle of an operation would change it in hard-to-see ways (e.g.
    ///   a merge that would conclude on another commit).
    /// - Invalid ref names (not under `refs/`, with spaces, control characters or
    ///   sequences forbidden by `git check-ref-format`) → [`EngineError::NotAllowed`]: they
    ///   never reach git.
    /// - HEAD that would change branch, or switch between branch and detached →
    ///   [`EngineError::NotAllowed`]: not yet supported (no journal command does it).
    pub fn check(&self, operation: Option<Operation>) -> Result<(), EngineError> {
        super::check_no_operation(self.command, msg!("engine-busy-restore"), operation)?;
        for r in &self.refs {
            if !valid_ref_name(&r.name) {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-journal-bad-ref",
                    name = r.name.escape_debug()
                )));
            }
        }
        match (&self.head_expected, &self.head_target) {
            (HeadState::Branch(a), HeadState::Branch(b)) if a == b => {}
            (HeadState::Detached(_), HeadState::Detached(_)) => {}
            _ => {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-restore-head-move",
                    command = self.command,
                    head_expected = self.head_expected,
                    head_target = self.head_target
                )));
            }
        }
        Ok(())
    }

    /// The differences between the expected and the current state (`head`, and `current` which gives the
    /// current value of a ref by name, `None` if it does not exist), one per line, for
    /// [`EngineError::UndoConflict`]. Empty if everything is as expected.
    pub fn conflicts(
        &self,
        head: &HeadState,
        mut current: impl FnMut(&str) -> Result<Option<Oid>, EngineError>,
    ) -> Result<Vec<Message>, EngineError> {
        let mut out = Vec::new();
        if *head != self.head_expected {
            out.push(msg!(
                "engine-head-changed",
                expected = self.head_expected,
                now = head
            ));
        }
        for r in &self.refs {
            let now = current(&r.name)?;
            if now != r.expected {
                out.push(msg!(
                    "engine-ref-changed",
                    name = r.name,
                    expected = describe(r.expected),
                    now = describe(now)
                ));
            }
        }
        Ok(out)
    }

    /// The commits that after the move would no longer be pointed to by HEAD or by the refs of the
    /// request: the expected values (of refs and detached HEAD) that are not among the new ones. The
    /// backend saves in a backup ref those that remain unreachable from other refs and from
    /// the new values (§7), so gc does not delete them and `edit.redo` remains possible.
    pub fn abandoned(&self) -> Vec<Oid> {
        let targets = self.targets();
        let mut out: Vec<Oid> = self
            .refs
            .iter()
            .filter_map(|r| r.expected)
            .chain(match self.head_expected {
                HeadState::Detached(oid) => Some(oid),
                HeadState::Branch(_) => None,
            })
            .filter(|oid| !targets.contains(oid))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The new values: of the refs and of detached HEAD.
    pub fn targets(&self) -> Vec<Oid> {
        self.refs
            .iter()
            .filter_map(|r| r.target)
            .chain(match self.head_target {
                HeadState::Detached(oid) => Some(oid),
                HeadState::Branch(_) => None,
            })
            .collect()
    }

    /// The message of the backup commit / of the backup ref's reflog for `oid`.
    pub fn backup_reflog(&self, oid: &Oid) -> String {
        format!(
            "project: backup of {oid} before {} ({})",
            self.command, self.reflog
        )
    }
}

/// Outcome of [`GitBackend::restore_refs`](crate::GitBackend::restore_refs).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreOutcome {
    /// The backup refs created for the commits that would no longer be reachable.
    pub backups: Vec<String>,
    /// Warnings about something unexpected found **after** the successful move (e.g. another
    /// process that changed HEAD right during the operation). Nothing is lost, but whoever
    /// calls must be informed. Usually empty.
    pub warnings: Vec<Message>,
}

/// True if `name` is a full ref name that git would accept (`git check-ref-format`) and that
/// can be passed to `update-ref --stdin -z` without ambiguity: under `refs/`, without spaces or
/// control characters, without `~ ^ : ? * [ \`, `..`, `@{`, empty components or ones starting
/// with `.` or ending with `.lock`, and not ending with `/` or `.`.
pub fn valid_ref_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("refs/") else {
        return false;
    };
    if rest.is_empty()
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("@{")
        || name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
    {
        return false;
    }
    rest.split('/')
        .all(|c| !c.is_empty() && !c.starts_with('.') && !c.ends_with(".lock"))
}

/// The first 7 digits of an id, like `git log --oneline`.
pub(crate) fn short(oid: &Oid) -> String {
    let hex = oid.to_string();
    hex[..7.min(hex.len())].to_owned()
}

fn describe(value: Option<Oid>) -> Message {
    match value {
        Some(oid) => msg!("engine-ref-value", id = short(&oid)),
        None => msg!("engine-ref-missing"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryBackend;

    fn oid(n: u64) -> Oid {
        MemoryBackend::fake_oid(n)
    }

    fn restore() -> RefRestore {
        RefRestore {
            command: "edit.undo",
            head_expected: HeadState::Branch("refs/heads/main".to_owned()),
            head_target: HeadState::Branch("refs/heads/main".to_owned()),
            refs: vec![RefMove {
                name: "refs/heads/main".to_owned(),
                expected: Some(oid(2)),
                target: Some(oid(1)),
            }],
            reflog: "project: undo x".to_owned(),
        }
    }

    #[test]
    fn ref_names() {
        for good in [
            "refs/heads/main",
            "refs/heads/feature/x-1",
            "refs/tags/v1.0",
        ] {
            assert!(valid_ref_name(good), "{good}");
        }
        for bad in [
            "HEAD",
            "main",
            "refs/",
            "refs/heads/",
            "refs/heads/a b",
            "refs/heads/a\nb",
            "refs/heads/a\0b",
            "refs/heads/a..b",
            "refs/heads/a@{1}",
            "refs/heads/.x",
            "refs/heads/x.lock",
            "refs/heads/x.",
            "refs/heads//x",
            "refs/heads/a*",
            "refs/heads/a:b",
        ] {
            assert!(!valid_ref_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn checks() {
        restore().check(None).unwrap();
        for op in [
            Operation::Merge,
            Operation::Rebase,
            Operation::Bisect,
            Operation::Am,
        ] {
            let err = restore().check(Some(op)).unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{op:?}");
        }
        let mut bad = restore();
        bad.refs[0].name = "refs/heads/a b".to_owned();
        assert!(matches!(bad.check(None), Err(EngineError::NotAllowed(_))));
        let mut switch = restore();
        switch.head_target = HeadState::Branch("refs/heads/another".to_owned());
        assert!(matches!(
            switch.check(None),
            Err(EngineError::NotAllowed(_))
        ));
        let mut detach = restore();
        detach.head_target = HeadState::Detached(oid(1));
        assert!(detach.check(None).is_err());
    }

    #[test]
    fn conflicts_and_abandoned() {
        let r = restore();
        let head = HeadState::Branch("refs/heads/main".to_owned());
        assert!(r.conflicts(&head, |_| Ok(Some(oid(2)))).unwrap().is_empty());
        let moved = r.conflicts(&head, |_| Ok(Some(oid(3)))).unwrap();
        assert_eq!(moved.len(), 1);
        assert!(moved[0].contains("refs/heads/main"), "{moved:?}");
        let gone = r.conflicts(&head, |_| Ok(None)).unwrap();
        assert!(gone[0].contains("missing"), "{gone:?}");
        let other = HeadState::Detached(oid(2));
        let both = r.conflicts(&other, |_| Ok(Some(oid(2)))).unwrap();
        assert!(both[0].starts_with("HEAD"), "{both:?}");

        assert_eq!(r.abandoned(), [oid(2)]);
        assert_eq!(r.targets(), [oid(1)]);
        let detached = RefRestore {
            head_expected: HeadState::Detached(oid(5)),
            head_target: HeadState::Detached(oid(4)),
            refs: Vec::new(),
            ..restore()
        };
        assert_eq!(detached.abandoned(), [oid(5)]);
        assert_eq!(detached.backup_kind(), "undo");
        let redo = RefRestore {
            command: "edit.redo",
            ..restore()
        };
        assert_eq!(redo.backup_kind(), "redo");
    }

    #[test]
    fn head_state_json() {
        let branch = HeadState::Branch("refs/heads/main".to_owned());
        assert_eq!(
            serde_json::to_value(&branch).unwrap(),
            serde_json::json!({ "branch": "refs/heads/main" })
        );
        let detached = HeadState::Detached(oid(1));
        assert_eq!(
            serde_json::to_value(&detached).unwrap(),
            serde_json::json!({ "detached": oid(1).to_string() })
        );
    }
}
