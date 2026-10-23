use eframe::wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
use project_engine::{Capabilities, MemoryOpener, Picked, Platform};
use std::sync::Arc;
use wasm_bindgen_futures::JsFuture;

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Debug).ok();

    wasm_bindgen_futures::spawn_local(async {
        let canvas = document()
            .get_element_by_id("app_canvas")
            .expect("manca #app_canvas")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("#app_canvas non è un canvas");

        let result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| {
                    // Il browser non vede i repo locali: repo di esempio in memoria, con un
                    // branch che va in conflitto con `main` per provare il merge.
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
            log::error!("avvio non riuscito: {e:?}");
        }
    });
}

fn document() -> web_sys::Document {
    web_sys::window()
        .and_then(|w| w.document())
        .expect("nessun document")
}

/// File tramite `<input type="file">` nascosto; salvataggio come download.
struct WebPlatform;

impl Platform for WebPlatform {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    /// Se l'utente annulla il dialogo il browser non avvisa: la callback
    /// non viene chiamata.
    fn pick_file(&self, on_done: Box<dyn FnOnce(Option<Picked>) + Send>) {
        if let Err(e) = pick_file(on_done) {
            log::error!("apertura file: {e:?}");
        }
    }

    /// Il browser non vede le cartelle locali: si usa il repo di esempio.
    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
        on_done(None);
    }

    /// `navigator.language`, es. `it-IT`.
    fn locale(&self) -> Option<String> {
        web_sys::window()?.navigator().language()
    }

    /// Il browser non ha thread: il job gira subito (il `MemoryBackend` è istantaneo).
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        job();
    }

    fn save_file(&self, suggested_name: &str, bytes: Vec<u8>) {
        if let Err(e) = download(suggested_name, &bytes) {
            log::error!("salvataggio file: {e:?}");
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
                    log::error!("lettura di {}: {e:?}", file.name());
                    None
                }
            };
            on_done(picked);
        });
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // La closure deve vivere finché il browser può chiamarla.
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
