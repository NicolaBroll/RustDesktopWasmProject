//! L7 — desktop app startup and native implementation of `Platform`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // no console on Windows in release

use project_engine::{Capabilities, Picked, Platform};
use project_git::CliOpener;
use std::sync::Arc;

/// Window icon (source in `assets/icon.svg`).
const ICON: &[u8] = include_bytes!("../../../assets/icon-256.png");

fn main() -> eframe::Result {
    env_logger::init();

    let platform = Arc::new(NativePlatform);
    let opener = Arc::new(CliOpener::new());
    // `project <path>` opens a repository right away. Absolute: the path is saved among the tabs
    // and the recent repos, and the next launch may start from another folder.
    let repo = std::env::args().nth(1).map(|path| {
        std::path::absolute(&path)
            .ok()
            .and_then(|p| p.into_os_string().into_string().ok())
            .unwrap_or(path)
    });
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
        Box::new(move |cc| Ok(Box::new(project_ui::App::new(cc, platform, opener, repo)))),
    )
}

/// System dialogs via `rfd`. They are blocking: the callback is
/// called before returning.
struct NativePlatform;

impl Platform for NativePlatform {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            window_control: true,
            open_folder: true,
            ..Default::default()
        }
    }

    fn pick_file(&self, on_done: Box<dyn FnOnce(Option<Picked>) + Send>) {
        let picked = rfd::FileDialog::new()
            .add_filter("Documento", &["json"])
            .pick_file()
            .and_then(|path| match std::fs::read(&path) {
                Ok(bytes) => Some(Picked {
                    name: path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    bytes,
                }),
                Err(e) => {
                    log::error!("{}: {e}", path.display());
                    None
                }
            });
        on_done(picked);
    }

    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
        let folder = rfd::FileDialog::new().pick_folder().and_then(|path| {
            let path = path.into_os_string().into_string();
            if let Err(path) = &path {
                log::error!("{}: the path is not UTF-8", path.to_string_lossy());
            }
            path.ok()
        });
        on_done(folder);
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

    fn save_file(&self, suggested_name: &str, bytes: Vec<u8>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Documento", &["json"])
            .set_file_name(suggested_name)
            .save_file()
        else {
            return;
        };
        if let Err(e) = std::fs::write(&path, bytes) {
            log::error!("{}: {e}", path.display());
        }
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
