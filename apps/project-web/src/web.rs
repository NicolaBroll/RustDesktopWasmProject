use eframe::wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
use project_engine::{Capabilities, MemoryOpener, Picked, Platform};
use std::sync::Arc;
use wasm_bindgen_futures::JsFuture;

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Debug).ok();

    wasm_bindgen_futures::spawn_local(async {
        let canvas = document()
            .get_element_by_id("app_canvas")
            .expect("missing #app_canvas")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("#app_canvas is not a canvas");

        let result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| {
                    // The browser cannot see local repos: in-memory sample repo, with a
                    // branch that conflicts with `main` to try the merge.
                    let opener = Arc::new(crate::demo::opener());
                    let repo = Some(MemoryOpener::SAMPLE_PATH.to_owned());
                    Ok(Box::new(project_ui::App::new(
                        cc,
                        Arc::new(WebPlatform),
                        opener,
                        repo,
                    )))
                }),
            )
            .await;
        if let Err(e) = result {
            log::error!("startup failed: {e:?}");
        }
    });
}

fn document() -> web_sys::Document {
    web_sys::window()
        .and_then(|w| w.document())
        .expect("no document")
}

/// File via a hidden `<input type="file">`; saving as a download.
struct WebPlatform;

impl Platform for WebPlatform {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    /// If the user cancels the dialog the browser does not notify: the callback
    /// is not called.
    fn pick_file(&self, on_done: Box<dyn FnOnce(Option<Picked>) + Send>) {
        if let Err(e) = pick_file(on_done) {
            log::error!("opening file: {e:?}");
        }
    }

    /// The browser cannot see local folders: the sample repo is used.
    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
        on_done(None);
    }

    /// The browser has no threads: the job runs immediately (the `MemoryBackend` is instant).
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        job();
    }

    fn save_file(&self, suggested_name: &str, bytes: Vec<u8>) {
        if let Err(e) = download(suggested_name, &bytes) {
            log::error!("saving file: {e:?}");
        }
    }
}

fn pick_file(on_done: Box<dyn FnOnce(Option<Picked>) + Send>) -> Result<(), JsValue> {
    let input: web_sys::HtmlInputElement = document().create_element("input")?.dyn_into()?;
    input.set_type("file");
    input.set_accept(".json,application/json");

    let target = input.clone();
    let on_change = Closure::once(move |_: web_sys::Event| {
        let Some(file) = target.files().and_then(|files| files.get(0)) else {
            on_done(None);
            return;
        };
        wasm_bindgen_futures::spawn_local(async move {
            let picked = match JsFuture::from(file.array_buffer()).await {
                Ok(buffer) => Some(Picked {
                    name: file.name(),
                    bytes: js_sys::Uint8Array::new(&buffer).to_vec(),
                }),
                Err(e) => {
                    log::error!("reading {}: {e:?}", file.name());
                    None
                }
            };
            on_done(picked);
        });
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The closure must live as long as the browser can call it.
    on_change.forget();
    input.click();
    Ok(())
}

fn download(name: &str, bytes: &[u8]) -> Result<(), JsValue> {
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/json");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;

    let anchor: web_sys::HtmlAnchorElement = document().create_element("a")?.dyn_into()?;
    anchor.set_href(&url);
    anchor.set_download(name);
    anchor.click();
    web_sys::Url::revoke_object_url(&url)
}
