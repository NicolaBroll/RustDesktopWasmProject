//! L7 — avvio dell'app desktop e implementazione nativa di `Platform`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // niente console su Windows in release

use project_engine::{Capabilities, Picked, Platform};
use project_git::CliOpener;
use std::sync::Arc;

/// Icona della finestra (sorgente in `assets/icon.svg`).
const ICON: &[u8] = include_bytes!("../../../assets/icon-256.png");

fn main() -> eframe::Result {
    env_logger::init();

    let platform = Arc::new(NativePlatform);
    let opener = Arc::new(CliOpener::new());
    // `project <percorso>` apre subito un repository.
    let repo = std::env::args().nth(1);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([640.0, 400.0])
            .with_icon(Arc::new(
                eframe::icon_data::from_png_bytes(ICON).expect("icona PNG valida"),
            )),
        ..Default::default()
    };
    eframe::run_native(
        "BinariGit",
        options,
        Box::new(move |cc| Ok(Box::new(project_ui::App::new(cc, platform, opener, repo)))),
    )
}

/// Dialoghi di sistema tramite `rfd`. Sono bloccanti: la callback viene
/// chiamata prima del ritorno.
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
                log::error!("{}: il percorso non è UTF-8", path.to_string_lossy());
            }
            path.ok()
        });
        on_done(folder);
    }

    /// Un thread per job: i comandi lenti sono pochi e brevi (fetch, push, log grandi).
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        if let Err(e) = std::thread::Builder::new()
            .name("project-job".to_owned())
            .spawn(job)
        {
            log::error!("impossibile avviare un thread: {e}");
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
