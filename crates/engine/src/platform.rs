//! Interfaccia verso la piattaforma ospite (desktop o browser).
//!
//! Le implementazioni vivono negli eseguibili (L7); la UI riceve un
//! `Arc<dyn Platform>` e non sa su quale target sta girando.

#[derive(Debug, Clone)]
pub struct Picked {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    pub print: bool,
    pub reveal_in_file_manager: bool,
    /// L'app può chiudere la propria finestra (falso nel browser).
    pub window_control: bool,
}

pub trait Platform: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    /// Asincrono sul web: il risultato arriva tramite callback, `None` se
    /// l'utente annulla.
    fn pick_file(&self, on_done: Box<dyn FnOnce(Option<Picked>) + Send>);
    fn save_file(&self, suggested_name: &str, bytes: Vec<u8>);
}
