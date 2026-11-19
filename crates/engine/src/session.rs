use crate::{
    CloneOutcome, CloneRequest, EngineError, GitBackend, InitRequest, JobContext, Journal,
    RepoOpener,
};
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
    /// True for a [`Session::reader`]: it gives the repo only for reading.
    reader: bool,
    /// How many times a repo was opened here ([`Session::opens`]).
    opens: u64,
}

struct OpenRepo {
    /// The path as it was passed to `repo.open`.
    path: String,
    backend: Box<dyn GitBackend>,
    journal: Journal,
}

impl Session {
    /// Session with no open repo. Desktop and CLI pass `CliOpener` from `project-git`, the
    /// tests a `MemoryOpener`.
    pub fn new(opener: Arc<dyn RepoOpener>) -> Self {
        Self {
            opener,
            repo: None,
            job: JobContext::new(),
            reader: false,
            opens: 0,
        }
    }

    /// A second session on the repo open here, only for reading it: the commands that read
    /// nothing else ([`CommandSpec::reader`](crate::CommandSpec::reader)) run on it while this
    /// session is held by a slow command (a fetch, a commit with a slow hook). The UI's executor
    /// uses it; CLI and MCP run one command at a time and do not need it.
    ///
    /// It has no journal and writes nothing: [`Session::repo_mut`], [`Session::journal`],
    /// [`Session::repo_and_journal`] and the functions that open a repo refuse with
    /// [`EngineError::NotAllowed`], so a command that is not a pure read fails there before
    /// changing anything. It reads through [`GitRead::reader`](crate::GitRead::reader), which may
    /// be a copy of the repo as it is now: take a new reader after every command that writes or
    /// opens. [`EngineError::NoRepo`] if no repo is open.
    pub fn reader(&self) -> Result<Session, EngineError> {
        let repo = self.repo.as_ref().ok_or(EngineError::NoRepo)?;
        Ok(Session {
            opener: Arc::clone(&self.opener),
            repo: Some(OpenRepo {
                path: repo.path.clone(),
                backend: repo.backend.reader(),
                journal: Journal::starting_at(0),
            }),
            job: JobContext::new(),
            reader: true,
            opens: 0,
        })
    }

    /// True for a session made by [`Session::reader`].
    pub fn is_reader(&self) -> bool {
        self.reader
    }

    /// How many times a repo was opened in this session (`repo.open`, `repo.clone`,
    /// `repo.init`): it grows when the open repo is replaced, even by the same one.
    pub fn opens(&self) -> u64 {
        self.opens
    }

    /// [`EngineError::NotAllowed`] on a [`Session::reader`], for what only the session that
    /// writes can do.
    fn check_writer(&self) -> Result<(), EngineError> {
        if self.reader {
            return Err(EngineError::NotAllowed(msg!("engine-reader-session")));
        }
        Ok(())
    }

    /// Opens the repo at `path` and replaces the open one. If opening fails the
    /// previously open repo stays as it is.
    pub fn open(&mut self, path: &str) -> Result<(), EngineError> {
        self.check_writer()?;
        let backend = self.opener.open(path)?;
        // Entry numbering continues: an id never refers to two different entries.
        let next_id = self.repo.as_ref().map_or(0, |r| r.journal.next_id());
        self.repo = Some(OpenRepo {
            path: path.to_owned(),
            backend,
            journal: Journal::starting_at(next_id),
        });
        self.opens += 1;
        Ok(())
    }

    /// `repo.clone`: clones with the opener (progress and cancellation from [`Session::job`]),
    /// then opens the clone as [`Session::open`] does. If opening fails the clone stays on
    /// disk and the previously open repo stays open.
    pub fn clone_repo(&mut self, request: &CloneRequest) -> Result<CloneOutcome, EngineError> {
        self.check_writer()?;
        let outcome = self.opener.clone_repo(request, &self.job)?;
        self.open(&request.path)?;
        Ok(outcome)
    }

    /// `repo.init`: creates the repo with the opener, then opens it as [`Session::open`] does.
    pub fn init_repo(&mut self, request: &InitRequest) -> Result<(), EngineError> {
        self.check_writer()?;
        self.opener.init_repo(request)?;
        self.open(&request.path)
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
        self.check_writer()?;
        match self.repo.as_mut() {
            Some(repo) => Ok(repo.backend.as_mut()),
            None => Err(EngineError::NoRepo),
        }
    }

    /// The journal of operations of the open repo; [`EngineError::NoRepo`] if there is none.
    pub fn journal(&self) -> Result<&Journal, EngineError> {
        self.check_writer()?;
        self.repo
            .as_ref()
            .map(|repo| &repo.journal)
            .ok_or(EngineError::NoRepo)
    }

    /// The open repo and its journal together, for the commands that write and record
    /// the operation (or undo it); [`EngineError::NoRepo`] if there is no repo.
    pub fn repo_and_journal(&mut self) -> Result<(&mut dyn GitBackend, &mut Journal), EngineError> {
        self.check_writer()?;
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

    fn refused(result: Result<impl Sized, EngineError>) -> bool {
        matches!(result, Err(EngineError::NotAllowed(m)) if m.key() == "engine-reader-session")
    }

    #[test]
    fn a_reader_reads_the_open_repo_and_refuses_the_rest() {
        let mut s = Session::new(Arc::new(MemoryOpener::sample()));
        assert!(matches!(s.reader(), Err(EngineError::NoRepo)));
        s.open(MemoryOpener::SAMPLE_PATH).unwrap();
        assert_eq!(s.opens(), 1);

        let mut reader = s.reader().unwrap();
        assert!(reader.is_reader() && !s.is_reader());
        assert_eq!(reader.path(), s.path());
        assert_eq!(
            reader.repo().unwrap().head().unwrap(),
            s.repo().unwrap().head().unwrap()
        );

        assert!(refused(reader.repo_mut()));
        assert!(refused(reader.journal()));
        assert!(refused(reader.repo_and_journal()));
        assert!(refused(reader.open(MemoryOpener::SAMPLE_PATH)));
        let init = InitRequest {
            path: MemoryOpener::SAMPLE_PATH.to_owned(),
            branch: None,
        };
        assert!(refused(reader.init_repo(&init)));
        // A reader of a reader is still one.
        assert!(reader.reader().unwrap().is_reader());
    }

    #[test]
    fn a_new_reader_sees_what_the_session_wrote() {
        use crate::CommitRequest;
        let mut s = Session::new(Arc::new(MemoryOpener::sample()));
        s.open(MemoryOpener::SAMPLE_PATH).unwrap();
        let before = s.reader().unwrap();
        let out = s
            .repo_mut()
            .unwrap()
            .commit(&CommitRequest::new("x"))
            .unwrap();

        let after = s.reader().unwrap();
        assert_eq!(
            after.repo().unwrap().head().unwrap().target(),
            Some(out.commit.id)
        );
        // The in-memory repo gives a copy: the reader taken before the write stays behind.
        assert_ne!(
            before.repo().unwrap().head().unwrap().target(),
            Some(out.commit.id)
        );

        // Opening again, even the same repo, counts.
        s.open(MemoryOpener::SAMPLE_PATH).unwrap();
        assert_eq!(s.opens(), 2);
        assert!(s.open("altrove").is_err());
        assert_eq!(s.opens(), 2);
    }
}
