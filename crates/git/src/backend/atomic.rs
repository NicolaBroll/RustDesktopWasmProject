//! Atomic writes of a whole worktree (or git directory) file: a new temporary file in the same
//! folder, synced to disk, then renamed over the file (or linked in place of a missing one).
//! Whoever reads the file sees either the old bytes or the new ones, never half of them.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// `.project-<pid>-<nanoseconds>.tmp` in the folder of `full`: without the file's name, which
/// could push it past the file system's name limit.
pub(super) fn temp_name(full: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    full.with_file_name(format!(".project-{}-{nanos}.tmp", std::process::id()))
}

/// Replaces the existing file `full` with `content`, keeping its permissions. If a step fails
/// the file is untouched and the temporary file removed.
pub(super) fn replace_file(full: &Path, content: &[u8]) -> std::io::Result<()> {
    let permissions = std::fs::metadata(full)?.permissions();
    let temp = temp_name(full);
    let written = write_temp(&temp, content, Some(permissions)).and_then(|()| {
        // `rename` replaces the file (on Windows too, `MoveFileEx` with replace).
        std::fs::rename(&temp, full)
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Creates the file `full` with `content`; fails with `AlreadyExists` if something is already
/// there (never overwrites). The temporary file is hard-linked in place, so the file appears
/// whole; on a file system without hard links it is written in place (`create_new`).
pub(super) fn create_file(full: &Path, content: &[u8]) -> std::io::Result<()> {
    let temp = temp_name(full);
    if let Err(e) = write_temp(&temp, content, None) {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    let linked = std::fs::hard_link(&temp, full);
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(full)?;
            let written = file.write_all(content).and_then(|()| file.sync_all());
            if written.is_err() {
                drop(file);
                let _ = std::fs::remove_file(full);
            }
            written
        }
    }
}

/// A new file `temp` (`create_new`) with `content` and `permissions`, synced to disk.
fn write_temp(
    temp: &Path,
    content: &[u8],
    permissions: Option<std::fs::Permissions>,
) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)?;
    file.write_all(content)?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_and_create() {
        let dir = std::env::temp_dir().join(format!("project-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f");
        let _ = std::fs::remove_file(&file);
        create_file(&file, b"one").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"one");
        let err = create_file(&file, b"two").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        replace_file(&file, b"three").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"three");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o640);
        }
        assert!(replace_file(&dir.join("missing"), b"x").is_err());
        // No temporary file left behind.
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["f"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn temp_name_is_short() {
        let long = "x".repeat(250);
        let name = temp_name(&Path::new("/d").join(long));
        let file = name.file_name().unwrap().to_string_lossy().into_owned();
        assert!(file.starts_with(".project-") && file.ends_with(".tmp") && file.len() < 50);
    }
}
