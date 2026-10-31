//! Interface to the host platform (the desktop app, or a fake one in tests).
//!
//! The implementations live in the executables (L7); the UI receives an
//! `Arc<dyn Platform>` and does not know which target it is running on.

use std::path::Path;
use std::sync::Arc;

/// Keeps a [`Platform::watch`] going: dropping it stops the watch.
pub type Watch = Box<dyn Send>;

/// Called by a [`Platform::watch`] when the repository changed, from any thread.
pub type OnChange = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    /// [`Platform::pick_folder`] opens a dialog.
    pub open_folder: bool,
    /// [`Platform::show_folder`] and [`Platform::open_terminal`] start the system's programs.
    pub external_apps: bool,
}

pub trait Platform: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    /// Asks the user for a folder; `None` if they cancel or if the platform has no folders
    /// (see [`Capabilities::open_folder`]). The path is a string because it is passed to
    /// `repo.open`: a folder with a non-UTF-8 name gives `None`. It does not block: `on_done` may
    /// be called later, from another thread (on desktop, when the dialog closes).
    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>);
    /// Runs `job` off the UI thread (see `impl-docs/ARCHITECTURE-git.md` §6): a
    /// thread on desktop; a test platform may run it immediately, before returning.
    fn spawn(&self, job: Box<dyn FnOnce() + Send>);
    /// Watches the git directory of the repository whose worktree root is `repo` (the `path` of
    /// `repo.open`) and calls `on_change`, debounced, when HEAD, the index, the refs or the
    /// operation in progress change: a commit from a terminal, an agent through MCP. The worktree
    /// is not watched (see `impl-docs/ARCHITECTURE-git.md` §8). `None`: the platform does not
    /// watch (the default, test platforms).
    fn watch(&self, repo: &str, on_change: OnChange) -> Option<Watch> {
        let _ = (repo, on_change);
        None
    }
    /// The language of the user's system as an ISO 639-1 code (`it`, `en`, ...), for the
    /// "as the system" UI language. `None`: not known (the default, test platforms); the UI then
    /// uses English.
    fn system_language(&self) -> Option<String> {
        None
    }
    /// Shows `folder` in the system's file manager, without waiting for it. The default (test
    /// platforms, see [`Capabilities::external_apps`]): [`std::io::ErrorKind::Unsupported`].
    fn show_folder(&self, folder: &Path) -> std::io::Result<()> {
        let _ = folder;
        Err(std::io::ErrorKind::Unsupported.into())
    }
    /// Opens a terminal in `folder`, without waiting for it. The default as [`Self::show_folder`].
    fn open_terminal(&self, folder: &Path) -> std::io::Result<()> {
        let _ = folder;
        Err(std::io::ErrorKind::Unsupported.into())
    }
}
