//! L7 — avvio dell'app nel browser e implementazione web di `Platform`.
//!
//! Compilato per un target diverso da wasm32 produce un eseguibile vuoto,
//! così `cargo build --workspace` funziona anche sul desktop.

// Fuori da wasm32 serve solo ai test.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod demo;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
fn main() {
    web::start();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("project-web va compilato per wasm32: usa `trunk serve` (vedi README).");
}
