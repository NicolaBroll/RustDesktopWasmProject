//! L7 — desktop app startup and native implementation of `Platform`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // no console on Windows in release

mod watch;

use project_engine::{Capabilities, OnChange, Platform, Watch};
use project_git::CliOpener;
use std::sync::Arc;

/// Window icon (source in `assets/icon.svg`).
const ICON: &[u8] = include_bytes!("../../../assets/icon-256.png");

fn main() -> eframe::Result {
    env_logger::init();

    let opener = Arc::new(CliOpener::new());
    // `project <path>` opens a repository right away (`repo.open` returns the absolute root, which
    // is what the tabs and the recent repos save).
    let repo = std::env::args().nth(1);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([640.0, 400.0])
            .with_icon(Arc::new(
                eframe::icon_data::from_png_bytes(ICON).expect("valid PNG icon"),
            )),
        ..Default::default()
    };
    eframe::run_native(
        "BinariGit",
        options,
        Box::new(move |cc| {
            // Built here: the dialogs need the window, which exists only now.
            let platform = Arc::new(NativePlatform::new(cc));
            Ok(Box::new(project_ui::App::new(cc, platform, opener, repo)))
        }),
    )
}

/// Folder dialog via `rfd`, threads for slow jobs, `notify` for the repos changed from outside.
struct NativePlatform {
    /// The dialog to open, parented to the main window (modal to it on Windows and macOS; on
    /// Linux the XDG portal, or zenity, receive the window id).
    dialog: rfd::AsyncFileDialog,
}

impl NativePlatform {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            dialog: rfd::AsyncFileDialog::new().set_parent(cc),
        }
    }
}

impl Platform for NativePlatform {
    fn capabilities(&self) -> Capabilities {
        Capabilities { open_folder: true }
    }

    /// The dialog is created here, on the UI thread (macOS wants the main thread), and shown by
    /// `rfd` without blocking it: on Linux and Windows on a thread of its own. A job thread waits
    /// for the result, so the window keeps repainting while the dialog is open.
    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
        let picking = self.dialog.clone().pick_folder();
        self.spawn(Box::new(move || {
            let folder = pollster::block_on(picking).and_then(|handle| {
                let path = handle.path().to_path_buf().into_os_string().into_string();
                if let Err(path) = &path {
                    log::error!("{}: the path is not UTF-8", path.to_string_lossy());
                }
                path.ok()
            });
            on_done(folder);
        }));
    }

    /// One thread per job: slow commands are few and short (fetch, push, large logs).
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        if let Err(e) = std::thread::Builder::new()
            .name("project-job".to_owned())
            .spawn(job)
        {
            log::error!("cannot start a thread: {e}");
        }
    }

    fn watch(&self, repo: &str, on_change: OnChange) -> Option<Watch> {
        watch::start(repo, on_change)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_is_valid_png() {
        let icon = eframe::icon_data::from_png_bytes(super::ICON).unwrap();
        assert_eq!((icon.width, icon.height), (256, 256));
    }
}
