//! Interface to the host platform (the desktop app, or a fake one in tests).
//!
//! The implementations live in the executables (L7); the UI receives an
//! `Arc<dyn Platform>` and does not know which target it is running on.

#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    /// [`Platform::pick_folder`] opens a dialog.
    pub open_folder: bool,
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
}
