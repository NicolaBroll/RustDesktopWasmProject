//! Confinamento dei percorsi nella radice consentita.

use std::io;
use std::path::{Component, Path, PathBuf};

pub(crate) struct Root(PathBuf);

impl Root {
    pub fn new(dir: &Path) -> io::Result<Self> {
        let dir = dir.canonicalize()?;
        if !dir.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} non è una cartella", dir.display()),
            ));
        }
        Ok(Self(dir))
    }

    /// Percorso di un file o di una cartella esistente, dentro la radice dopo aver risolto i
    /// link. `.` è la radice stessa.
    pub fn existing(&self, path: &str) -> Result<PathBuf, String> {
        let full = self.join(path)?;
        let canonical = full.canonicalize().map_err(|e| format!("{path}: {e}"))?;
        self.check(path, canonical)
    }

    fn join(&self, path: &str) -> Result<PathBuf, String> {
        let rel = Path::new(path);
        if rel.is_absolute() || rel.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(format!(
                "{path}: usa un percorso relativo alla radice, senza `..`"
            ));
        }
        Ok(self.0.join(rel))
    }

    fn check(&self, original: &str, resolved: PathBuf) -> Result<PathBuf, String> {
        if resolved.starts_with(&self.0) {
            Ok(resolved)
        } else {
            Err(format!("{original}: fuori dalla radice consentita"))
        }
    }
}
