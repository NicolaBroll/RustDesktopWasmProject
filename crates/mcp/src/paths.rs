//! Confining paths to the allowed root.

use std::io;
use std::path::{Component, Path, PathBuf};

pub(crate) struct Root(PathBuf);

impl Root {
    pub fn new(dir: &Path) -> io::Result<Self> {
        let dir = dir.canonicalize()?;
        if !dir.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a folder", dir.display()),
            ));
        }
        Ok(Self(dir))
    }

    /// Path of an existing file or folder, inside the root after resolving links. `.` is the
    /// root itself.
    pub fn existing(&self, path: &str) -> Result<PathBuf, String> {
        let full = self.join(path)?;
        let canonical = full.canonicalize().map_err(|e| format!("{path}: {e}"))?;
        self.check(path, canonical)
    }

    fn join(&self, path: &str) -> Result<PathBuf, String> {
        let rel = Path::new(path);
        if rel.is_absolute() || rel.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(format!(
                "{path}: use a path relative to the root, without `..`"
            ));
        }
        Ok(self.0.join(rel))
    }

    fn check(&self, original: &str, resolved: PathBuf) -> Result<PathBuf, String> {
        if resolved.starts_with(&self.0) {
            Ok(resolved)
        } else {
            Err(format!("{original}: outside the allowed root"))
        }
    }
}
