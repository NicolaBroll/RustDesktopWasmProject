//! L7 — desktop app startup and native implementation of `Platform`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // no console on Windows in release

mod watch;

use project_engine::{Capabilities, OnChange, Platform, Watch};
use project_git::CliOpener;
use std::sync::Arc;

/// Window icon (source in `assets/icon.svg`).
const ICON: &[u8] = include_bytes!("../../../assets/icon-256.png");

fn main() -> eframe::Result {
    // Started by git or ssh as askpass (S52): ask the running app and exit, never start.
    if let Some(code) = project_git::askpass_client() {
        std::process::exit(code);
    }
    env_logger::init();

    // Network commands ask for credentials through the UI.
    // The dialog gets the window's context when the App is created.
    let credentials = Arc::new(project_ui::CredentialDialog::new());
    let opener = Arc::new(CliOpener::new().credential_prompter(credentials.clone()));
    // `project <path>` opens a repository right away (`repo.open` returns the absolute root, which
    // is what the tabs and the recent repos save).
    let repo = std::env::args().nth(1);
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1200.0, 760.0])
        .with_min_inner_size([640.0, 400.0])
        .with_icon(Arc::new(
            eframe::icon_data::from_png_bytes(ICON).expect("valid PNG icon"),
        ));
    // The menus are in the title bar, VS Code style: on macOS over a transparent title bar with
    // the native buttons, elsewhere without decorations and with the app's own buttons.
    let (viewport, frame) = if cfg!(target_os = "macos") {
        let viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false);
        (viewport, project_ui::WindowFrame::MacOs)
    } else {
        (
            viewport.with_decorations(false),
            project_ui::WindowFrame::Custom,
        )
    };
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "BinariGit",
        options,
        Box::new(move |cc| {
            // Built here: the dialogs need the window, which exists only now.
            let platform = Arc::new(NativePlatform::new(cc));
            Ok(Box::new(
                project_ui::App::new(cc, platform, opener, repo)
                    .with_window_frame(frame)
                    .with_credentials(credentials.connect(&cc.egui_ctx)),
            ))
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

    fn system_language(&self) -> Option<String> {
        language_from_env(|name| std::env::var(name).ok())
    }
}

/// The language of the POSIX locale variables, in their order of precedence (`LC_ALL`,
/// `LC_MESSAGES`, `LANG`): `it_IT.UTF-8` gives `it`. `C`/`POSIX` and empty values say nothing.
/// Windows and macOS usually do not set them: `None`, and the UI shows English.
fn language_from_env(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| {
            let value = var(name)?;
            let code: String = value
                .chars()
                .take_while(char::is_ascii_alphabetic)
                .collect::<String>()
                .to_ascii_lowercase();
            (code.len() >= 2 && code != "c" && code != "posix").then_some(code)
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_is_valid_png() {
        let icon = eframe::icon_data::from_png_bytes(super::ICON).unwrap();
        assert_eq!((icon.width, icon.height), (256, 256));
    }

    #[test]
    fn system_language_comes_from_the_locale_variables() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let lang = super::language_from_env;
        assert_eq!(lang(env(&[("LANG", "it_IT.UTF-8")])).as_deref(), Some("it"));
        assert_eq!(
            lang(env(&[("LC_ALL", "en_US.UTF-8"), ("LANG", "it_IT.UTF-8")])).as_deref(),
            Some("en")
        );
        // `C` says nothing: the next variable decides.
        assert_eq!(
            lang(env(&[("LC_ALL", "C"), ("LANG", "it_IT")])).as_deref(),
            Some("it")
        );
        assert_eq!(lang(env(&[("LANG", "C.UTF-8")])), None);
        assert_eq!(lang(env(&[("LANG", "")])), None);
        assert_eq!(lang(env(&[])), None);
    }
}
