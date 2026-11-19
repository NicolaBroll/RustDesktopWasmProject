//! Telling a folder dialog that could not be shown from one the user closed.
//!
//! `rfd` answers `None` for both. On Linux it asks the XDG desktop portal and, when the portal
//! does not answer, starts `zenity`; when zenity cannot be started either it logs an error from
//! its portal module and gives up at once. That record is the only sign `rfd` gives, and it is
//! written before the answer arrives: the app's logger counts them, and a `None` during which
//! the count grew is a dialog that never opened. A dialog closed by the user logs nothing, on
//! any system.

use project_engine::{NoDialog, Picked};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// The module of `rfd` that falls back to zenity: its errors are "zenity could not be run".
const RFD_FALLBACK: &str = "rfd::backend::xdg_desktop_portal";

/// What shows the dialog where `rfd` gives up: one of the two is to be installed.
const NEEDS: [&str; 2] = ["xdg-desktop-portal", "zenity"];

/// How many times `rfd` gave up on a dialog since the app started.
static GAVE_UP: AtomicU64 = AtomicU64::new(0);

fn gave_up(level: log::Level, target: &str) -> bool {
    level == log::Level::Error && target == RFD_FALLBACK
}

/// The logger of `env_logger` (`RUST_LOG`), which also counts the dialogs `rfd` gave up on.
struct Logger {
    inner: env_logger::Logger,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        gave_up(metadata.level(), metadata.target()) || self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        if gave_up(record.level(), record.target()) {
            GAVE_UP.fetch_add(1, Ordering::Release);
        }
        // It applies the filters of `RUST_LOG` itself.
        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Installs the logger: as `env_logger::init`, but errors always reach it (also with
/// `RUST_LOG=off`, which then prints nothing), so the count above is kept.
pub(crate) fn init_logger() {
    let inner = env_logger::Builder::from_default_env().build();
    let level = inner.filter().max(log::LevelFilter::Error);
    if log::set_boxed_logger(Box::new(Logger { inner })).is_ok() {
        log::set_max_level(level);
    }
}

/// The count to read before a dialog is started and to give to [`outcome`] with its answer.
pub(crate) fn mark() -> u64 {
    GAVE_UP.load(Ordering::Acquire)
}

/// What the answer of `rfd` means: the folder, a dialog closed without a choice (also a folder
/// whose name is not UTF-8), or, if `rfd` gave up since `mark`, a dialog that never opened.
pub(crate) fn outcome(chosen: Option<PathBuf>, mark: u64) -> Picked {
    match chosen {
        Some(path) => match path.into_os_string().into_string() {
            Ok(path) => Ok(Some(path)),
            Err(path) => {
                log::error!("{}: the path is not UTF-8", path.to_string_lossy());
                Ok(None)
            }
        },
        None if self::mark() != mark => Err(NoDialog {
            needs: NEEDS.map(str::to_owned).to_vec(),
        }),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_error_of_the_zenity_fallback_counts() {
        assert!(gave_up(log::Level::Error, RFD_FALLBACK));
        // "Using zenity fallback": zenity may still show the dialog.
        assert!(!gave_up(log::Level::Warn, RFD_FALLBACK));
        // The portal that does not answer: zenity is tried next.
        let portal = "rfd::backend::xdg_desktop_portal::portal";
        assert!(!gave_up(log::Level::Error, portal));
        assert!(!gave_up(log::Level::Error, "project"));
    }

    #[test]
    fn a_folder_or_a_closed_dialog() {
        let mark = mark();
        let chosen = outcome(Some(PathBuf::from("/work/repo")), mark);
        assert_eq!(chosen, Ok(Some("/work/repo".to_owned())));
        assert_eq!(outcome(None, mark), Ok(None));
    }

    /// The real thing, where it can run without showing a dialog to anyone: no display, no
    /// session bus and no zenity (a container, most CI machines). It holds the tie with `rfd`'s
    /// log record: after an update of `rfd` that words it differently this fails.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_dialog_that_cannot_open_is_not_a_closed_one() {
        let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        let zenity = std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|d| d.join("zenity").is_file()));
        let bus = set("DBUS_SESSION_BUS_ADDRESS")
            || std::env::var_os("XDG_RUNTIME_DIR")
                .is_some_and(|dir| PathBuf::from(dir).join("bus").exists());
        if zenity || bus || set("DISPLAY") || set("WAYLAND_DISPLAY") {
            eprintln!("skipped: a folder dialog could open here");
            return;
        }
        init_logger();
        let mark = mark();
        let chosen = pollster::block_on(rfd::AsyncFileDialog::new().pick_folder());
        let chosen = chosen.map(|handle| handle.path().to_path_buf());
        let Err(no_dialog) = outcome(chosen, mark) else {
            panic!("rfd gave up without the error this module counts");
        };
        assert_eq!(no_dialog.needs, NEEDS);
    }
}
