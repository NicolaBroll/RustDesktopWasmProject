use crate::{EngineError, GitBackend, RepoOpener};
use std::fmt;
use std::sync::Arc;

/// La sessione di lavoro su un repository: l'opener iniettato dall'app e il repo aperto.
///
/// I comandi del [`Registry`](crate::Registry) la ricevono come `&mut Session`; `repo.open`
/// apre un repo con l'opener, gli altri comandi leggono quello aperto.
pub struct Session {
    opener: Arc<dyn RepoOpener>,
    repo: Option<OpenRepo>,
}

struct OpenRepo {
    /// Il percorso come è stato passato a `repo.open`.
    path: String,
    backend: Box<dyn GitBackend>,
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
        self.repo = Some(OpenRepo {
            path: path.to_owned(),
            backend,
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
}
