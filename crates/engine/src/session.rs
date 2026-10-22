use crate::{EngineError, GitBackend, Journal, RepoOpener};
use std::fmt;
use std::sync::Arc;

/// La sessione di lavoro su un repository: l'opener iniettato dall'app, il repo aperto e il
/// suo diario delle operazioni ([`Journal`], per `edit.undo`/`edit.redo`).
///
/// I comandi del [`Registry`](crate::Registry) la ricevono come `&mut Session`; `repo.open`
/// apre un repo con l'opener, gli altri comandi leggono quello aperto. Il diario è in memoria
/// e appartiene al repo aperto: aprire un repo (anche lo stesso) ne parte uno vuoto.
pub struct Session {
    opener: Arc<dyn RepoOpener>,
    repo: Option<OpenRepo>,
}

struct OpenRepo {
    /// Il percorso come è stato passato a `repo.open`.
    path: String,
    backend: Box<dyn GitBackend>,
    journal: Journal,
}

impl Session {
    /// Sessione senza repo aperto. Desktop e CLI passano `CliOpener` di `project-git`, la
    /// versione web e i test un [`MemoryOpener`](crate::MemoryOpener).
    pub fn new(opener: Arc<dyn RepoOpener>) -> Self {
        Self { opener, repo: None }
    }

    /// Apre il repo al percorso `path` e sostituisce quello aperto. Se l'apertura fallisce il
    /// repo aperto prima resta com'è.
    pub fn open(&mut self, path: &str) -> Result<(), EngineError> {
        let backend = self.opener.open(path)?;
        // La numerazione delle voci continua: un id non indica mai due voci diverse.
        let next_id = self.repo.as_ref().map_or(0, |r| r.journal.next_id());
        self.repo = Some(OpenRepo {
            path: path.to_owned(),
            backend,
            journal: Journal::starting_at(next_id),
        });
        Ok(())
    }

    /// Il percorso del repo aperto, `None` se non ce n'è uno.
    pub fn path(&self) -> Option<&str> {
        self.repo.as_ref().map(|repo| repo.path.as_str())
    }

    /// Il repo aperto; [`EngineError::NoRepo`] se non ce n'è uno.
    pub fn repo(&self) -> Result<&dyn GitBackend, EngineError> {
        self.repo
            .as_ref()
            .map(|repo| repo.backend.as_ref())
            .ok_or(EngineError::NoRepo)
    }

    /// Il repo aperto, per i comandi che lo modificano; [`EngineError::NoRepo`] se non ce n'è
    /// uno.
    pub fn repo_mut(&mut self) -> Result<&mut dyn GitBackend, EngineError> {
        match self.repo.as_mut() {
            Some(repo) => Ok(repo.backend.as_mut()),
            None => Err(EngineError::NoRepo),
        }
    }

    /// Il diario delle operazioni del repo aperto; [`EngineError::NoRepo`] se non ce n'è uno.
    pub fn journal(&self) -> Result<&Journal, EngineError> {
        self.repo
            .as_ref()
            .map(|repo| &repo.journal)
            .ok_or(EngineError::NoRepo)
    }

    /// Il repo aperto e il suo diario insieme, per i comandi che scrivono e registrano
    /// l'operazione (o la annullano); [`EngineError::NoRepo`] se non c'è un repo.
    pub fn repo_and_journal(&mut self) -> Result<(&mut dyn GitBackend, &mut Journal), EngineError> {
        match self.repo.as_mut() {
            Some(repo) => Ok((repo.backend.as_mut(), &mut repo.journal)),
            None => Err(EngineError::NoRepo),
        }
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

        // Un'apertura non riuscita lascia repo e diario.
        assert!(s.open("altrove").is_err());
        assert_eq!(s.journal().unwrap().done().len(), 1);

        // Riaprire (anche lo stesso repo) azzera il diario; gli id continuano.
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
