use crate::{EngineError, GitBackend, JobContext, Journal, RepoOpener};
use std::fmt;
use std::sync::Arc;

/// The working session on a repository: the opener injected by the app, the open repo and its
/// journal of operations ([`Journal`], for `edit.undo`/`edit.redo`).
///
/// The commands of the [`Registry`](crate::Registry) receive it as `&mut Session`; `repo.open`
/// opens a repo with the opener, the other commands read the open one. The journal is in memory
/// and belongs to the open repo: opening a repo (even the same one) starts an empty one.
pub struct Session {
    opener: Arc<dyn RepoOpener>,
    repo: Option<OpenRepo>,
    /// Progress and cancellation of the running command: the one passed to
    /// [`Registry::run_with`](crate::Registry::run_with), empty outside a command and with
    /// [`Registry::run`](crate::Registry::run).
    job: JobContext,
}

struct OpenRepo {
    /// The path as it was passed to `repo.open`.
    path: String,
    backend: Box<dyn GitBackend>,
    journal: Journal,
}

impl Session {
    /// Session with no open repo. Desktop and CLI pass `CliOpener` from `project-git`, the
    /// tests a [`MemoryOpener`](crate::MemoryOpener).
    pub fn new(opener: Arc<dyn RepoOpener>) -> Self {
        Self {
            opener,
            repo: None,
            job: JobContext::new(),
        }
    }

    /// Opens the repo at `path` and replaces the open one. If opening fails the
    /// previously open repo stays as it is.
    pub fn open(&mut self, path: &str) -> Result<(), EngineError> {
        let backend = self.opener.open(path)?;
        // Entry numbering continues: an id never refers to two different entries.
        let next_id = self.repo.as_ref().map_or(0, |r| r.journal.next_id());
        self.repo = Some(OpenRepo {
            path: path.to_owned(),
            backend,
            journal: Journal::starting_at(next_id),
        });
        Ok(())
    }

    /// The path of the open repo, `None` if there is none.
    pub fn path(&self) -> Option<&str> {
        self.repo.as_ref().map(|repo| repo.path.as_str())
    }

    /// The open repo; [`EngineError::NoRepo`] if there is none.
    pub fn repo(&self) -> Result<&dyn GitBackend, EngineError> {
        self.repo
            .as_ref()
            .map(|repo| repo.backend.as_ref())
            .ok_or(EngineError::NoRepo)
    }

    /// The open repo, for the commands that modify it; [`EngineError::NoRepo`] if there is
    /// none.
    pub fn repo_mut(&mut self) -> Result<&mut dyn GitBackend, EngineError> {
        match self.repo.as_mut() {
            Some(repo) => Ok(repo.backend.as_mut()),
            None => Err(EngineError::NoRepo),
        }
    }

    /// The journal of operations of the open repo; [`EngineError::NoRepo`] if there is none.
    pub fn journal(&self) -> Result<&Journal, EngineError> {
        self.repo
            .as_ref()
            .map(|repo| &repo.journal)
            .ok_or(EngineError::NoRepo)
    }

    /// The open repo and its journal together, for the commands that write and record
    /// the operation (or undo it); [`EngineError::NoRepo`] if there is no repo.
    pub fn repo_and_journal(&mut self) -> Result<(&mut dyn GitBackend, &mut Journal), EngineError> {
        match self.repo.as_mut() {
            Some(repo) => Ok((repo.backend.as_mut(), &mut repo.journal)),
            None => Err(EngineError::NoRepo),
        }
    }
}

impl Session {
    /// Progress and cancellation of the running command (§6): slow commands pass it to the
    /// backend. Empty (no progress, never cancelled) if the command was launched with
    /// [`Registry::run`](crate::Registry::run).
    pub fn job(&self) -> &JobContext {
        &self.job
    }

    /// The context of the command about to start (or, afterwards, the empty one).
    pub(crate) fn set_job(&mut self, job: JobContext) {
        self.job = job;
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("path", &self.path())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryOpener;

    #[test]
    fn opens_a_repo_and_keeps_it_on_failure() {
        let mut s = Session::new(Arc::new(MemoryOpener::sample()));
        assert_eq!(s.path(), None);
        assert!(matches!(s.repo(), Err(EngineError::NoRepo)));

        s.open(MemoryOpener::SAMPLE_PATH).unwrap();
        assert_eq!(s.path(), Some(MemoryOpener::SAMPLE_PATH));
        assert!(s.repo().unwrap().head().unwrap().target().is_some());

        let err = s.open("altrove").unwrap_err();
        assert!(matches!(err, EngineError::RepoNotFound { .. }), "{err}");
        assert_eq!(s.path(), Some(MemoryOpener::SAMPLE_PATH));
    }

    #[test]
    fn the_journal_belongs_to_the_open_repo() {
        use crate::{CommitRequest, JournalEntry};
        let mut s = Session::new(Arc::new(MemoryOpener::sample()));
        assert!(matches!(s.journal(), Err(EngineError::NoRepo)));
        s.open(MemoryOpener::SAMPLE_PATH).unwrap();
        let (repo, journal) = s.repo_and_journal().unwrap();
        let out = repo.commit(&CommitRequest::new("x")).unwrap();
        journal.record(JournalEntry::commit(&out, false).unwrap());
        assert!(s.journal().unwrap().can_undo());

        // A failed open leaves repo and journal untouched.
        assert!(s.open("altrove").is_err());
        assert_eq!(s.journal().unwrap().done().len(), 1);

        // Reopening (even the same repo) resets the journal; ids continue.
        s.open(MemoryOpener::SAMPLE_PATH).unwrap();
        assert!(!s.journal().unwrap().can_undo());
        let (repo, journal) = s.repo_and_journal().unwrap();
        let out = repo.commit(&CommitRequest::new("y")).unwrap();
        assert_eq!(
            journal.record(JournalEntry::commit(&out, false).unwrap()),
            2
        );
    }
}
