//! `repo.clone` and `repo.init` on the [`CliOpener`] (`impl-docs/git/clone.md`), following §5
//! and §6:
//!
//! - the destination is absent or an empty folder, never a file, a link or a folder with something
//!   in it; an absent one is created here with `create_dir` (atomic), so git always receives an
//!   existing empty folder and the command knows which folder is its own;
//! - `git clone --progress [--branch=<b>] [--depth=<n>] --end-of-options <url> <abs path>`: a URL
//!   starting with `-` is never an option; `-c protocol.ext.allow=never -c protocol.fd.allow=never`
//!   as for fetch; run with `Git::run_network` (stdin closed, `GIT_TERMINAL_PROMPT=0`, no
//!   controlling terminal, credentials asked through the prompter if set), progress to the `JobContext`, cancellable;
//! - on cancellation or error the destination goes back to how it was: a folder created here is
//!   removed, a pre-existing empty folder is emptied again (all it holds is git's). Not when git says
//!   the destination is not empty: someone else wrote there in the meantime.

use super::CliOpener;
use crate::command::{Segment, failure};
use crate::progress::parse_progress;
use project_engine::{
    CloneOutcome, CloneRequest, EngineError, InitRequest, JobContext, clone_no_such_branch,
    destination_no_parent, destination_not_empty, destination_not_folder,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;

/// The destination folder of a clone or init, checked and ready (existing and empty).
struct Destination {
    /// The path as given, for the messages.
    given: String,
    /// Absolute (against the current directory), UTF-8.
    path: PathBuf,
    /// The folder was created by the command (it did not exist).
    created: bool,
}

impl Destination {
    /// Checks `path` and creates it if absent. Nothing is written if it is refused.
    fn prepare(path: &str) -> Result<Self, EngineError> {
        let other = |e: io::Error| {
            EngineError::NotAllowed(msg!("engine-other", text = format!("{path}: {e}")))
        };
        let abs = std::path::absolute(path).map_err(other)?;
        // `..` resolved as the file system does (through links), so the folder created and checked
        // here is the one git receives.
        let abs = match (abs.parent(), abs.file_name()) {
            (Some(parent), Some(name)) => match fs::canonicalize(parent) {
                Ok(parent) => parent.join(name),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(destination_no_parent(path));
                }
                Err(e) => return Err(other(e)),
            },
            _ => fs::canonicalize(&abs).map_err(other)?,
        };
        if abs.to_str().is_none() {
            // Only with a non-UTF-8 current directory: git receives the path as text.
            return Err(other(io::Error::other("not UTF-8")));
        }
        let created = match fs::symlink_metadata(&abs) {
            Ok(meta) if meta.file_type().is_dir() => {
                if fs::read_dir(&abs).map_err(other)?.next().is_some() {
                    return Err(destination_not_empty(path));
                }
                false
            }
            Ok(_) => return Err(destination_not_folder(path)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::create_dir(&abs) {
                Ok(()) => true,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(destination_no_parent(path));
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    return Err(destination_not_empty(path));
                }
                Err(e) => return Err(other(e)),
            },
            Err(e) => return Err(other(e)),
        };
        Ok(Self {
            given: path.to_owned(),
            path: abs,
            created,
        })
    }

    fn as_str(&self) -> &str {
        self.path.to_str().expect("checked in prepare")
    }

    /// Puts the destination back as it was before the command: removes the folder it created,
    /// or empties the pre-existing one (it was empty). Links inside are removed, not followed.
    fn restore(&self) -> io::Result<()> {
        if self.created {
            return match fs::remove_dir_all(&self.path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            };
        }
        // Still the real folder of the check, not a link put in its place meanwhile.
        if !fs::symlink_metadata(&self.path)?.file_type().is_dir() {
            return Err(io::Error::other("the destination is no longer a folder"));
        }
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    /// `error` after cleaning up the destination; a failed cleanup is added to the text.
    fn fail(&self, error: EngineError) -> EngineError {
        match self.restore() {
            Ok(()) => error,
            Err(e) => EngineError::GitWithReason {
                command: "clone".to_owned(),
                reason: msg!(
                    "engine-clone-cleanup-failed",
                    error = error.to_string(),
                    path = self.given.as_str(),
                    reason = e.to_string()
                ),
            },
        }
    }
}

/// Where git runs: the current directory, so a relative local URL means what it means for the user.
fn working_dir(dest: &Path) -> PathBuf {
    std::env::current_dir()
        .ok()
        .or_else(|| dest.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("/"))
}

impl CliOpener {
    /// [`RepoOpener::clone_repo`](project_engine::RepoOpener::clone_repo).
    pub(super) fn clone_into(
        &self,
        request: &CloneRequest,
        job: &JobContext,
    ) -> Result<CloneOutcome, EngineError> {
        self.check_version()?;
        let dest = Destination::prepare(&request.path)?;
        let branch = request.branch.as_ref().map(|b| format!("--branch={b}"));
        let depth = request.depth.map(|n| format!("--depth={n}"));
        let mut args = vec![
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "protocol.fd.allow=never",
            "clone",
            "--progress",
        ];
        args.extend(branch.as_deref());
        args.extend(depth.as_deref());
        args.extend(["--end-of-options", &request.url, dest.as_str()]);

        let mut lines = Vec::new();
        let streamed = self.git.run_network(
            &working_dir(&dest.path),
            &args,
            false,
            &request.url,
            |Segment { text, newline }| match parse_progress(&text) {
                Some(progress) => job.report(&progress),
                None if newline && !text.trim().is_empty() => {
                    lines.push(text.trim_end().to_owned())
                }
                None => {}
            },
            &|| job.is_cancelled(),
        );
        let streamed = match streamed {
            Ok(streamed) => streamed,
            Err(e) => return Err(dest.fail(e)),
        };
        if streamed.cancelled {
            return Err(dest.fail(EngineError::Cancelled {
                command: "repo.clone",
            }));
        }
        if !streamed.status.success() {
            let stderr = lines.join("\n");
            let lower = stderr.to_ascii_lowercase();
            if lower.contains("already exists and is not an empty directory") {
                // Something appeared in the folder after the check: not ours to remove.
                return Err(destination_not_empty(&request.path));
            }
            let error = if let (Some(branch), true) =
                (&request.branch, lower.contains("not found in upstream"))
            {
                clone_no_such_branch(&request.url, branch, stderr.trim())
            } else if let Some(error) = streamed.remote_error(&request.url, &stderr) {
                error
            } else if lower.contains("repository '") && lower.contains("' does not exist") {
                EngineError::RemoteUnreachable {
                    remote: request.url.clone(),
                    message: stderr.trim().to_owned(),
                }
            } else {
                failure(
                    &args,
                    &Output {
                        status: streamed.status,
                        stdout: Vec::new(),
                        stderr: stderr.into_bytes(),
                    },
                )
            };
            return Err(dest.fail(error));
        }
        let messages = lines
            .into_iter()
            // Git's own statistics, not something to show.
            .filter(|l| {
                !l.starts_with("Cloning into ") && l != "done." && !l.starts_with("remote: Total ")
            })
            .collect();
        Ok(CloneOutcome { messages })
    }

    /// [`RepoOpener::init_repo`](project_engine::RepoOpener::init_repo).
    pub(super) fn init_into(&self, request: &InitRequest) -> Result<(), EngineError> {
        self.check_version()?;
        let dest = Destination::prepare(&request.path)?;
        let branch = request
            .branch
            .as_ref()
            .map(|b| format!("--initial-branch={b}"));
        let mut args = vec!["init", "--quiet"];
        args.extend(branch.as_deref());
        args.extend(["--", dest.as_str()]);
        match self.git.output(Some(&working_dir(&dest.path)), &args) {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => Err(dest.fail(failure(&args, &out))),
            Err(e) => Err(dest.fail(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Destination;
    use std::fs;

    #[test]
    fn restore_removes_only_what_the_command_added() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "mine").unwrap();

        // Created by the command: removed whole.
        let new = tmp.path().join("new");
        let dest = Destination::prepare(new.to_str().unwrap()).unwrap();
        assert!(dest.created && new.is_dir());
        fs::create_dir_all(new.join(".git/objects")).unwrap();
        dest.restore().unwrap();
        assert!(!new.exists());

        // Pre-existing and empty: emptied again, the folder stays; a link inside is removed,
        // not followed.
        let existing = tmp.path().join("existing");
        fs::create_dir(&existing).unwrap();
        let dest = Destination::prepare(existing.to_str().unwrap()).unwrap();
        assert!(!dest.created);
        fs::create_dir_all(existing.join(".git/refs")).unwrap();
        fs::write(existing.join("file"), "git's").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, existing.join("link")).unwrap();
        dest.restore().unwrap();
        assert!(existing.is_dir());
        assert_eq!(fs::read_dir(&existing).unwrap().count(), 0);
        assert_eq!(
            fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "mine"
        );
    }
}
