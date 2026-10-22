//! L7 — avvio dell'app desktop e implementazione nativa di `Platform`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // niente console su Windows in release

use project_engine::{Capabilities, Picked, Platform};
use project_git::CliOpener;
use std::sync::Arc;

fn main() -> eframe::Result {
    env_logger::init();

    let platform = Arc::new(NativePlatform);
    let opener = Arc::new(CliOpener::new());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([300.0, 200.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Project",
        options,
        Box::new(move |cc| Ok(Box::new(project_ui::App::new(cc, platform, opener)))),
    )
}

/// Dialoghi di sistema tramite `rfd`. Sono bloccanti: la callback viene
/// chiamata prima del ritorno.
struct NativePlatform;

impl Platform for NativePlatform {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            window_control: true,
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
