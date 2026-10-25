//! Commit message in the stage area: the draft (subject, body, amend), when the
//! "Commit" button is active and why not, the parameters of `commit.create`, the answer and the
//! errors. Data only, no egui: tested with unit tests.
//!
//! The draft is UI state, not persistent: it stays while the repo is open, even if a
//! commit is selected and then you go back to the changes row, and it is not lost if the commit
//! fails. It is emptied only after a successful commit.

use crate::executor::ErrorKind;
use crate::{Action, Done, Request, Tab};
use project_domain::{Commit, Oid, Operation, Status};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

/// Beyond this length the subject is flagged with a mild warning (git convention: the
/// log lines stay readable).
pub const SUBJECT_LIMIT: usize = 72;

/// The message being written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitDraft {
    /// Subject, on one line.
    pub subject: String,
    /// Body, also on several lines; can be empty.
    pub body: String,
    /// Rewrites the last commit instead of creating a new one.
    amend: bool,
    /// The message of the last commit proposed by turning on amend with an empty draft: if
    /// amend is turned off without changing it, the draft goes back to empty.
    proposed: Option<(String, String)>,
}

impl CommitDraft {
    pub fn amend(&self) -> bool {
        self.amend
    }

    /// The message to send: the subject, then a blank line and the body if any. Trailing spaces
    /// and extra blank lines are removed by git (`commit.cleanup`).
    pub fn message(&self) -> String {
        let subject = self.subject.trim_end();
        let body = self.body.trim_end();
        if body.trim().is_empty() {
            subject.to_owned()
        } else {
            format!("{subject}\n\n{body}")
        }
    }

    /// True if the message is empty or only spaces.
    pub fn is_blank(&self) -> bool {
        self.subject.trim().is_empty() && self.body.trim().is_empty()
    }

    /// Characters of the subject (its first line, without trailing spaces).
    pub fn subject_len(&self) -> usize {
        self.subject
            .lines()
            .next()
            .unwrap_or("")
            .trim_end()
            .chars()
            .count()
    }

    pub fn subject_too_long(&self) -> bool {
        self.subject_len() > SUBJECT_LIMIT
    }

    /// Turns amend on or off. Turning it on with an empty draft proposes the message of
    /// `last` (the last commit, if known); turning it off removes the proposed message if it
    /// was not changed. Returns `true` if the message of the last commit is needed and `last`
    /// was not there: it must be requested and then passed to [`CommitDraft::propose`].
    pub fn set_amend(&mut self, on: bool, last: Option<&Commit>) -> bool {
        self.amend = on;
        if !on {
            if let Some((subject, body)) = self.proposed.take()
                && subject == self.subject
                && body == self.body
            {
                self.subject.clear();
                self.body.clear();
            }
            return false;
        }
        match last {
            Some(commit) => {
                self.propose(commit);
                false
            }
            None => self.is_blank(),
        }
    }

    /// Proposes the message of `commit` (the last one) if amend is on and the draft is empty.
    pub fn propose(&mut self, commit: &Commit) {
        if self.amend && self.is_blank() {
            self.subject = commit.summary.clone();
            self.body = commit.body.clone();
            self.proposed = Some((self.subject.clone(), self.body.clone()));
        }
    }

    /// True if the draft is the proposed message, not yet changed.
    pub fn is_proposed(&self) -> bool {
        self.proposed
            .as_ref()
            .is_some_and(|(s, b)| *s == self.subject && *b == self.body)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Why the "Commit" button is disabled; the text explains what to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// A write (even a commit) is in progress, or a confirmation is open.
    Busy,
    /// Conflicted files.
    Conflicts,
    /// Merge, rebase, cherry-pick or revert in progress.
    Operation(Operation),
    /// Amend without a commit to rewrite (branch without commits).
    NoCommitToAmend,
    /// Nothing staged, without amend.
    NothingStaged,
    /// Empty message or only spaces.
    NoMessage,
}

impl Blocked {
    pub fn hint(self) -> String {
        match self {
            Blocked::Busy => tr!("commit-blocked-busy"),
            Blocked::Conflicts => tr!("commit-blocked-conflicts"),
            Blocked::Operation(Operation::Merge) => tr!("commit-blocked-merge"),
            Blocked::Operation(Operation::Rebase) => tr!("commit-blocked-rebase"),
            Blocked::Operation(Operation::CherryPick) => tr!("commit-blocked-cherry-pick"),
            Blocked::Operation(Operation::Revert) => tr!("commit-blocked-revert"),
            Blocked::Operation(Operation::Am) => tr!("commit-blocked-am"),
            Blocked::Operation(_) => tr!("commit-blocked-operation"),
            Blocked::NoCommitToAmend => tr!("commit-blocked-no-commit"),
            Blocked::NothingStaged => tr!("commit-blocked-nothing-staged"),
            Blocked::NoMessage => tr!("commit-blocked-no-message"),
        }
    }
}

/// What is needed to decide whether the "Commit" button is active.
#[derive(Debug, Clone, Copy)]
pub struct Gate<'a> {
    pub draft: &'a CommitDraft,
    pub status: Option<&'a Status>,
    /// How many files are in the "Staged" list.
    pub staged: usize,
    /// True if HEAD points to a commit.
    pub has_head: bool,
    /// A write in progress or a confirmation open.
    pub busy: bool,
}

impl Gate<'_> {
    /// `Ok` if it is possible to commit, otherwise the first reason why not.
    pub fn check(&self) -> Result<(), Blocked> {
        if self.busy {
            return Err(Blocked::Busy);
        }
        // A merge in progress is concluded with `merge.continue` (the operation box),
        // which enters the journal as a merge; the commit box stays for normal commits.
        match self.status {
            Some(status) if status.has_conflicts() => return Err(Blocked::Conflicts),
            // During a bisect you commit normally.
            Some(Status {
                operation: Some(op),
                ..
            }) if *op != Operation::Bisect => return Err(Blocked::Operation(*op)),
            _ => {}
        }
        if self.draft.amend() {
            if !self.has_head {
                return Err(Blocked::NoCommitToAmend);
            }
        } else if self.staged == 0 {
            return Err(Blocked::NothingStaged);
        }
        if self.draft.is_blank() {
            return Err(Blocked::NoMessage);
        }
        Ok(())
    }
}

/// A commit sent (or to be sent) to `commit.create`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSend {
    pub message: String,
    pub amend: bool,
}

impl CommitSend {
    pub fn new(draft: &CommitDraft) -> Self {
        Self {
            message: draft.message(),
            amend: draft.amend(),
        }
    }

    /// The Registry command id.
    pub fn command(&self) -> &'static str {
        "commit.create"
    }

    /// The parameters of `commit.create`; `confirm` only after the user's confirmation.
    pub fn params(&self, confirm: bool) -> Value {
        let mut params = json!({ "message": self.message });
        if self.amend {
            params["amend"] = json!(true);
        }
        if confirm {
            params["confirm"] = json!(true);
        }
        params
    }

    /// Name of the operation for the status bar: "Committing…".
    pub fn label(&self) -> String {
        if self.amend {
            tr!("commit-amend-label")
        } else {
            tr!("commit-label")
        }
    }
}

/// Result of `commit.create` (the `branch` field is not needed by the UI: refs are reread).
#[derive(Debug, Clone, Deserialize)]
pub struct CommitResult {
    pub commit: Commit,
    /// HEAD before the commit (with amend, the rewritten commit).
    pub previous: Option<Oid>,
    /// With amend, the backup ref of the rewritten commit.
    pub backup: Option<String>,
    /// With amend, the other branches and tags left on the rewritten commit.
    #[serde(default)]
    pub also_on: Vec<String>,
    pub status: Status,
}

impl CommitResult {
    /// The message for the status bar.
    pub fn notice(&self) -> String {
        // Amend that produced the same commit: the engine changed nothing.
        if self.previous == Some(self.commit.id) && self.backup.is_none() {
            return tr!("commit-unchanged", id = self.commit.id.short());
        }
        let id = self.commit.id.short();
        let mut text = match (self.previous, &self.backup) {
            (Some(previous), Some(backup)) => tr!(
                "commit-amended",
                id = id,
                previous = previous.short(),
                backup = backup
            ),
            (None, Some(backup)) => tr!("commit-created-backup", id = id, backup = backup),
            (_, None) => tr!("commit-created", id = id),
        };
        if !self.also_on.is_empty() {
            text.push_str(&tr!("commit-also-on", refs = self.also_on.join(", ")));
        }
        text
    }
}

/// The other local branches and tags that point exactly at `head` (full names): with amend
/// they stay on the old commit. Only those pointing directly at HEAD, from the refs already read;
/// the full list (also the refs further ahead) arrives in `also_on` after the amend.
pub fn refs_left_behind(
    refs: &[project_domain::RefInfo],
    head: Option<Oid>,
    branch: Option<&str>,
) -> Vec<String> {
    use project_domain::RefKind;
    let Some(head) = head else {
        return Vec::new();
    };
    refs.iter()
        .filter(|r| r.target == head)
        .filter(|r| match r.kind {
            RefKind::LocalBranch => Some(r.name.as_str()) != branch,
            RefKind::Tag => true,
            _ => false,
        })
        .map(|r| r.name.clone())
        .collect()
}

/// An amend of an already published commit, waiting for the user's confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmAmend {
    pub send: CommitSend,
    /// The reason given by the engine (e.g. the remote branches containing the commit).
    pub reason: String,
}

/// The error of the last commit, shown under the message box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitError {
    pub message: String,
    /// The full git output (hook, signature, identity), to show in a scrollable area.
    pub output: Option<String>,
}

/// The message box: the draft, the commit in progress, the confirmation and the last error.
#[derive(Debug, Clone, Default)]
pub struct CommitBox {
    pub draft: CommitDraft,
    sending: Option<CommitSend>,
    confirm: Option<ConfirmAmend>,
    error: Option<CommitError>,
}

impl CommitBox {
    /// The commit in progress, if any.
    pub fn sending(&self) -> Option<&CommitSend> {
        self.sending.as_ref()
    }

    pub fn confirm(&self) -> Option<&ConfirmAmend> {
        self.confirm.as_ref()
    }

    pub fn error(&self) -> Option<&CommitError> {
        self.error.as_ref()
    }

    pub fn dismiss_error(&mut self) {
        self.error = None;
    }

    /// Prepares the commit of the draft: returns the command to send and the parameters.
    /// `None` if there is already a commit in progress or a confirmation open.
    pub fn start(&mut self) -> Option<(CommitSend, Value)> {
        if self.sending.is_some() || self.confirm.is_some() {
            return None;
        }
        let send = CommitSend::new(&self.draft);
        Some(self.begin(send, false))
    }

    /// The user confirmed the amend: the same commit, with `"confirm": true`.
    pub fn confirmed(&mut self) -> Option<(CommitSend, Value)> {
        let ConfirmAmend { send, .. } = self.confirm.take()?;
        Some(self.begin(send, true))
    }

    pub fn cancel_confirm(&mut self) {
        self.confirm = None;
    }

    fn begin(&mut self, send: CommitSend, confirm: bool) -> (CommitSend, Value) {
        self.error = None;
        let params = send.params(confirm);
        self.sending = Some(send.clone());
        (send, params)
    }

    /// The commit succeeded: the draft is emptied (and amend is turned off).
    pub fn succeeded(&mut self) {
        self.sending = None;
        self.error = None;
        self.draft.clear();
    }

    /// The commit succeeded but the engine reports a problem afterwards (`PostCommit`): the draft is
    /// emptied as after a success (the commit must not be repeated) and the error is shown.
    pub fn done_with_error(&mut self, message: &str) {
        self.succeeded();
        self.error = Some(CommitError {
            message: format!("{message}."),
            output: None,
        });
    }

    /// The commit failed. The draft stays as it was. With [`ErrorKind::ConfirmRequired`] the
    /// confirmation opens; other errors are shown under the box.
    pub fn failed(&mut self, kind: Option<ErrorKind>, message: &str, detail: Option<String>) {
        let send = self.sending.take();
        match (kind, send) {
            (Some(ErrorKind::ConfirmRequired), Some(send)) => {
                self.confirm = Some(ConfirmAmend {
                    send,
                    reason: detail.unwrap_or_else(|| message.to_owned()),
                });
            }
            _ => self.error = Some(error(kind, message, detail)),
        }
    }
}

/// The error text for the user; the written message is never lost.
fn error(kind: Option<ErrorKind>, message: &str, detail: Option<String>) -> CommitError {
    match kind {
        Some(ErrorKind::Git) => CommitError {
            message: tr!("commit-git-failed"),
            output: Some(
                detail
                    .filter(|out| !out.trim().is_empty())
                    .unwrap_or_else(|| tr!("commit-no-output")),
            ),
        },
        _ => CommitError {
            message: tr!("commit-failed-kept", error = message),
            output: None,
        },
    }
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_commit(&mut self, action: Action) {
        match action {
            Action::SetAmend(on) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if repo.commit.sending().is_some() {
                    return;
                }
                let last = repo.head_commit().cloned();
                let ask = repo.commit.draft.set_amend(on, last.as_ref());
                if ask && let Some(head) = repo.head.target() {
                    let params = json!({ "id": head.to_string() });
                    self.submit("commit.show", params, Request::HeadMessage(generation));
                }
            }
            Action::Commit => self.send_commit(false),
            Action::ConfirmCommit => self.send_commit(true),
            Action::CancelCommit => {
                if let Some(repo) = &mut self.repo {
                    repo.commit.cancel_confirm();
                }
            }
            Action::DismissCommitError => {
                if let Some(repo) = &mut self.repo {
                    repo.commit.dismiss_error();
                }
            }
            _ => unreachable!("action dispatched to the wrong function"),
        }
    }

    /// Sends the commit of the draft (`confirmed`: the one waiting for confirmation, with
    /// `"confirm": true`), if the button is active: never together with another write.
    pub(crate) fn send_commit(&mut self, confirmed: bool) {
        let writing = self.executor.writing().is_some();
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        let request = if confirmed {
            if writing {
                return;
            }
            repo.commit.confirmed()
        } else {
            if repo.commit_gate(writing).check().is_err() {
                return;
            }
            repo.commit.start()
        };
        let Some((send, params)) = request else {
            return;
        };
        repo.write_error = None;
        repo.write_label = Some(send.label());
        self.submit(send.command(), params, Request::Commit(generation));
    }

    /// Result of `commit.create`: state from the answer, log and refs reread; or the error
    /// (or the confirmation) in the box, with the written message kept.
    pub(crate) fn receive_commit(&mut self, generation: u64, done: Done) {
        let result = done.result.and_then(|value| {
            serde_json::from_value::<CommitResult>(value).map_err(|e| {
                tr!(
                    "commit-unexpected-result",
                    command = &done.command,
                    error = e.to_string()
                )
            })
        });
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match result {
            Ok(result) => {
                repo.commit.succeeded();
                let notice = result.notice();
                let id = result.commit.id;
                repo.set_status(result.status);
                // With other changes you stay on the changes row; if it is gone the
                // new commit is selected as soon as the log arrives.
                if !repo.worktree_selected {
                    repo.select_after_load = Some(id);
                }
                self.error.clear();
                self.notice = notice;
                self.follow_diff();
                // The new commit on top (with amend the old one disappears) and the shifted
                // ref labels.
                self.submit("repo.refs", json!({}), Request::Refs(generation));
                // The commit is in the journal: Undo now proposes it.
                self.submit("edit.history", json!({}), Request::History(generation));
                if let Some(repo) = self.current(generation) {
                    let first = repo.list.reset();
                    self.request_page(first);
                }
            }
            Err(e) if done.error_kind == Some(ErrorKind::PostCommit) => {
                // The commit is there: everything is reread, and the message is not proposed again.
                log::warn!("{}: {e}", done.command);
                repo.commit.done_with_error(&e);
                self.error = e;
                self.notice.clear();
                self.load();
            }
            Err(e) => {
                log::warn!("{}: {e}", done.command);
                repo.commit
                    .failed(done.error_kind, &e, done.error_detail.clone());
                if done.error_kind == Some(ErrorKind::ConfirmRequired) {
                    self.error.clear();
                } else {
                    self.error = tr!("commit-failed", error = &e);
                }
                self.notice.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Change, EntryKind, Head, Signature, StatusEntry, UnmergedInfo};

    fn commit(summary: &str, body: &str) -> Commit {
        let sig = Signature {
            name: "Ada Rossi".to_owned(),
            email: "ada@example.com".to_owned(),
            time: 0,
            offset_min: 0,
        };
        Commit {
            id: Oid::from_bytes([7; 20]),
            parents: Vec::new(),
            author: sig.clone(),
            committer: sig,
            summary: summary.to_owned(),
            body: body.to_owned(),
        }
    }

    fn draft(subject: &str, body: &str) -> CommitDraft {
        CommitDraft {
            subject: subject.to_owned(),
            body: body.to_owned(),
            ..CommitDraft::default()
        }
    }

    fn status(entries: Vec<StatusEntry>, operation: Option<Operation>) -> Status {
        Status {
            head: Head::Branch {
                name: "main".to_owned(),
                target: None,
            },
            upstream: None,
            entries,
            operation,
        }
    }

    fn conflict() -> StatusEntry {
        let zero = Oid::from_bytes([0; 20]);
        StatusEntry {
            path: "a.rs".into(),
            index: Change::Unmerged,
            worktree: Change::Unmerged,
            kind: EntryKind::Unmerged(UnmergedInfo {
                conflict: project_domain::Conflict::BothModified,
                submodule: None,
                mode_base: 0o100644,
                mode_ours: 0o100644,
                mode_theirs: 0o100644,
                mode_worktree: 0o100644,
                oid_base: zero,
                oid_ours: zero,
                oid_theirs: zero,
            }),
        }
    }

    #[test]
    fn message_joins_subject_and_body() {
        assert_eq!(draft("Subject  ", "").message(), "Subject");
        assert_eq!(draft("Subject", " \n\n").message(), "Subject");
        assert_eq!(
            draft("Subject", "Line 1\n\nLine 2\n\n").message(),
            "Subject\n\nLine 1\n\nLine 2"
        );
        // Without a subject the body passes as is: git uses its first line as the subject.
        assert_eq!(draft("", "body only").message(), "\n\nbody only");
        assert!(draft(" ", "\n\t").is_blank());
        assert!(!draft("", "x").is_blank());
    }

    #[test]
    fn subject_counter_counts_characters() {
        assert_eq!(draft("na\u{ef}ve", "").subject_len(), 5);
        assert_eq!(draft("abc   ", "").subject_len(), 3);
        assert!(!draft(&"a".repeat(72), "").subject_too_long());
        assert!(draft(&"\u{e9}".repeat(73), "").subject_too_long());
    }

    #[test]
    fn amend_proposes_the_last_message() {
        let last = commit("Last commit", "With a body");
        let mut d = CommitDraft::default();
        assert!(!d.set_amend(true, Some(&last)));
        assert!(d.amend());
        assert_eq!(
            (d.subject.as_str(), d.body.as_str()),
            ("Last commit", "With a body")
        );
        assert!(d.is_proposed());
        // Turned off without changing it: the draft goes back to empty.
        d.set_amend(false, Some(&last));
        assert!(d.is_blank());
        assert!(!d.amend());

        // Changed: it stays.
        d.set_amend(true, Some(&last));
        d.subject.push_str(" fixed");
        assert!(!d.is_proposed());
        d.set_amend(false, None);
        assert_eq!(d.subject, "Last commit fixed");

        // With a message already written nothing is proposed.
        let mut d = draft("Mine", "");
        d.set_amend(true, Some(&last));
        assert_eq!(d.subject, "Mine");
        assert!(!d.is_proposed());

        // Last commit not loaded: it must be requested, then proposed.
        let mut d = CommitDraft::default();
        assert!(d.set_amend(true, None));
        d.propose(&last);
        assert_eq!(d.subject, "Last commit");
        // Arrived after amend was turned off: nothing.
        let mut d = CommitDraft::default();
        d.set_amend(true, None);
        d.set_amend(false, None);
        d.propose(&last);
        assert!(d.is_blank());
    }

    #[test]
    fn the_button_explains_why_it_is_disabled() {
        let ready = draft("Message", "");
        let clean = status(Vec::new(), None);
        let gate = |d: &CommitDraft, s: &Status, staged, busy| {
            Gate {
                draft: d,
                status: Some(s),
                staged,
                has_head: true,
                busy,
            }
            .check()
        };
        assert_eq!(gate(&ready, &clean, 1, false), Ok(()));
        assert_eq!(gate(&ready, &clean, 1, true), Err(Blocked::Busy));
        assert_eq!(gate(&ready, &clean, 0, false), Err(Blocked::NothingStaged));
        assert_eq!(
            gate(&draft("  ", "\n"), &clean, 1, false),
            Err(Blocked::NoMessage)
        );
        let conflicts = status(vec![conflict()], Some(Operation::Merge));
        assert_eq!(gate(&ready, &conflicts, 1, false), Err(Blocked::Conflicts));
        let rebase = status(Vec::new(), Some(Operation::Rebase));
        assert_eq!(
            gate(&ready, &rebase, 1, false),
            Err(Blocked::Operation(Operation::Rebase))
        );
        let am = status(Vec::new(), Some(Operation::Am));
        assert_eq!(
            gate(&ready, &am, 1, false),
            Err(Blocked::Operation(Operation::Am))
        );
        // Merge resolved: it is concluded from the operation box, not with a commit.
        let merge = status(Vec::new(), Some(Operation::Merge));
        assert_eq!(
            gate(&ready, &merge, 0, false),
            Err(Blocked::Operation(Operation::Merge))
        );

        let mut amend = ready.clone();
        amend.set_amend(true, None);
        assert_eq!(gate(&amend, &clean, 0, false), Ok(()));
        assert_eq!(
            gate(&amend, &merge, 0, false),
            Err(Blocked::Operation(Operation::Merge))
        );
        let no_head = Gate {
            draft: &amend,
            status: Some(&clean),
            staged: 0,
            has_head: false,
            busy: false,
        };
        assert_eq!(no_head.check(), Err(Blocked::NoCommitToAmend));
        for blocked in [Blocked::Busy, Blocked::NothingStaged, Blocked::NoMessage] {
            assert!(!blocked.hint().is_empty());
        }
    }

    #[test]
    fn notice_after_commit_and_amend() {
        let status = status(Vec::new(), None);
        let c = commit("x", "");
        let other = Oid::from_bytes([8; 20]);
        let result = |previous, backup: Option<&str>, also_on: Vec<&str>| CommitResult {
            commit: c.clone(),
            previous,
            backup: backup.map(str::to_owned),
            also_on: also_on.into_iter().map(str::to_owned).collect(),
            status: status.clone(),
        };
        let id = c.id.short();
        assert_eq!(
            result(Some(other), None, vec![]).notice(),
            format!("Commit {id} created")
        );
        let amend = result(
            Some(other),
            Some("refs/project/backup/amend-1"),
            vec!["refs/tags/v1"],
        );
        assert_eq!(
            amend.notice(),
            format!(
                "Commit {id} created in place of {}; backup of the previous commit in \
                 refs/project/backup/amend-1; the previous commit is still on refs/tags/v1",
                other.short()
            )
        );
        let same = result(Some(c.id), None, vec![]);
        assert!(same.notice().starts_with("No change"), "{}", same.notice());
    }

    #[test]
    fn refs_left_behind_by_an_amend() {
        use project_domain::{RefInfo, RefKind};
        let head = Oid::from_bytes([1; 20]);
        let r = |name: &str, kind, target| RefInfo {
            name: name.to_owned(),
            kind,
            target,
            tag_object: None,
            upstream: None,
        };
        let refs = [
            r("refs/heads/other", RefKind::LocalBranch, head),
            r("refs/heads/main", RefKind::LocalBranch, head),
            r(
                "refs/heads/old",
                RefKind::LocalBranch,
                Oid::from_bytes([2; 20]),
            ),
            r("refs/remotes/origin/main", RefKind::RemoteBranch, head),
            r("refs/tags/v1", RefKind::Tag, head),
        ];
        assert_eq!(
            refs_left_behind(&refs, Some(head), Some("refs/heads/main")),
            ["refs/heads/other", "refs/tags/v1"]
        );
        assert!(refs_left_behind(&refs, None, None).is_empty());
    }

    #[test]
    fn params_carry_amend_and_confirm_only_when_set() {
        let mut d = draft("Subject", "Body");
        let send = CommitSend::new(&d);
        assert_eq!(send.command(), "commit.create");
        assert_eq!(send.params(false), json!({ "message": "Subject\n\nBody" }));
        assert_eq!(send.label(), "Commit");
        d.set_amend(true, None);
        let send = CommitSend::new(&d);
        assert_eq!(
            send.params(true),
            json!({ "message": "Subject\n\nBody", "amend": true, "confirm": true })
        );
        assert_eq!(send.label(), "Amending the last commit");
    }

    #[test]
    fn errors_keep_the_draft() {
        let mut b = CommitBox {
            draft: draft("Subject", ""),
            ..CommitBox::default()
        };
        let (_, params) = b.start().unwrap();
        assert_eq!(params, json!({ "message": "Subject" }));
        assert!(b.sending().is_some());
        assert!(b.start().is_none(), "never two commits at once");

        b.failed(
            Some(ErrorKind::Git),
            "`git commit` failed: ...",
            Some("pre-commit: found TODO\nin a.rs:3".to_owned()),
        );
        assert!(b.sending().is_none());
        assert_eq!(b.draft.subject, "Subject");
        let error = b.error().unwrap();
        assert!(error.message.contains("hook"), "{}", error.message);
        assert_eq!(
            error.output.as_deref(),
            Some("pre-commit: found TODO\nin a.rs:3")
        );

        b.start().unwrap();
        assert!(b.error().is_none(), "a new attempt clears the error");
        b.failed(Some(ErrorKind::NothingToCommit), "nothing to commit", None);
        let error = b.error().unwrap();
        assert!(error.message.contains("nothing to commit"));
        assert!(error.message.contains("kept"));
        assert_eq!(error.output, None);
        b.dismiss_error();
        assert!(b.error().is_none());

        b.start().unwrap();
        b.failed(Some(ErrorKind::Git), "x", Some("  \n".to_owned()));
        assert_eq!(b.error().unwrap().output.as_deref(), Some("(no output)"));

        // Commit succeeded with a problem afterwards: it must not be repeated, the draft empties.
        b.start().unwrap();
        b.done_with_error("commit abc was created, but HEAD changed");
        assert!(b.draft.is_blank());
        assert!(b.sending().is_none());
        assert!(b.error().unwrap().message.contains("was created"));
    }

    #[test]
    fn a_published_amend_asks_then_resends_with_confirm() {
        let mut b = CommitBox::default();
        b.draft.subject = "Rewrite".to_owned();
        b.draft.set_amend(true, None);
        b.start().unwrap();
        b.failed(
            Some(ErrorKind::ConfirmRequired),
            "asks for confirmation",
            Some("the commit is already on refs/remotes/origin/main".to_owned()),
        );
        let confirm = b.confirm().unwrap();
        assert_eq!(
            confirm.reason,
            "the commit is already on refs/remotes/origin/main"
        );
        assert!(b.error().is_none());
        assert!(b.start().is_none(), "no commit with the confirmation open");

        // Cancelled: nothing to send, the draft stays.
        b.cancel_confirm();
        assert!(b.confirm().is_none());
        assert!(b.draft.amend());

        b.start().unwrap();
        b.failed(Some(ErrorKind::ConfirmRequired), "m", Some("r".to_owned()));
        let (send, params) = b.confirmed().unwrap();
        assert!(send.amend);
        assert_eq!(
            params,
            json!({ "message": "Rewrite", "amend": true, "confirm": true })
        );
        b.succeeded();
        assert!(b.draft.is_blank());
        assert!(!b.draft.amend());
        assert!(b.sending().is_none());
    }

    /// The full path with the sample repo of the `MemoryBackend`, as the UI does it:
    /// stage of note.txt, commit, then the log and the status from the answer; finally amend.
    #[test]
    fn commit_and_amend_with_the_sample_repo() {
        use crate::stage::StageLists;
        use project_engine::{EngineError, MemoryOpener, Registry, Session};
        use std::sync::Arc;

        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(MemoryOpener::sample()));
        let path = MemoryOpener::SAMPLE_PATH;
        registry
            .run("repo.open", &mut session, json!({ "path": path }))
            .unwrap();
        let out = registry
            .run(
                "index.stage",
                &mut session,
                json!({ "paths": ["note.txt"] }),
            )
            .unwrap();
        let status: Status = serde_json::from_value(out["status"].clone()).unwrap();
        let lists = StageLists::new(&status);
        let head = status.head.target();

        let mut b = CommitBox::default();
        b.draft.subject = "Adds the notes".to_owned();
        b.draft.body = "With README and guide.".to_owned();
        let gate = Gate {
            draft: &b.draft,
            status: Some(&status),
            staged: lists.staged.len(),
            has_head: head.is_some(),
            busy: false,
        };
        assert_eq!(gate.check(), Ok(()));
        let (send, params) = b.start().unwrap();
        let out = registry.run(send.command(), &mut session, params).unwrap();
        let result: CommitResult = serde_json::from_value(out).unwrap();
        b.succeeded();
        assert!(b.draft.is_blank());
        assert_eq!(result.previous, head);
        assert_eq!(result.backup, None);
        assert_eq!(result.commit.summary, "Adds the notes");
        assert_eq!(result.commit.author.name, "Ada Rossi");
        assert_eq!(
            result.notice(),
            format!("Commit {} created", result.commit.id.short())
        );
        // The state from the answer: nothing more staged.
        let lists = StageLists::new(&result.status);
        assert!(lists.staged.is_empty());
        assert_eq!(result.status.head.target(), Some(result.commit.id));
        // The reloaded log has the new commit on top.
        let log = registry
            .run("repo.log", &mut session, json!({ "limit": 2 }))
            .unwrap();
        let log: Vec<Commit> = serde_json::from_value(log["commits"].clone()).unwrap();
        assert_eq!(log[0], result.commit);

        // Another commit with nothing staged: the button is disabled, and the engine
        // would refuse anyway.
        b.draft.subject = "Again".to_owned();
        let gate = Gate {
            draft: &b.draft,
            status: Some(&result.status),
            staged: 0,
            has_head: true,
            busy: false,
        };
        assert_eq!(gate.check(), Err(Blocked::NothingStaged));
        let err = registry
            .run(
                "commit.create",
                &mut session,
                CommitSend::new(&b.draft).params(false),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NothingToCommit), "{err}");
        b.failed(
            Some(ErrorKind::of(&err)),
            &err.text(),
            ErrorKind::detail(&err),
        );
        assert_eq!(b.draft.subject, "Again", "the message is not lost");

        // Amend: the message of the last commit is proposed, then changed.
        b.draft.clear();
        b.draft.set_amend(true, Some(&log[0]));
        assert_eq!(b.draft.subject, "Adds the notes");
        b.draft.subject = "Adds the work notes".to_owned();
        let (send, params) = b.start().unwrap();
        assert_eq!(params["amend"], true);
        let out = registry.run(send.command(), &mut session, params).unwrap();
        let amended: CommitResult = serde_json::from_value(out).unwrap();
        b.succeeded();
        assert_eq!(amended.previous, Some(result.commit.id));
        let backup = amended
            .backup
            .clone()
            .expect("backup of the rewritten commit");
        assert!(amended.notice().contains(&backup), "{}", amended.notice());
        assert_eq!(amended.commit.parents, result.commit.parents);
        let log = registry
            .run("repo.log", &mut session, json!({ "limit": 2 }))
            .unwrap();
        let log: Vec<Commit> = serde_json::from_value(log["commits"].clone()).unwrap();
        // The old commit disappears from the log, the new one is on top.
        assert_eq!(log[0].id, amended.commit.id);
        assert_eq!(log[0].summary, "Adds the work notes");
        assert!(log.iter().all(|c| c.id != result.commit.id));
    }
}
