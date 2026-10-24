//! L7 — browser app startup and web implementation of `Platform`.
//!
//! Compiled for a target other than wasm32 it produces an empty executable,
//! so `cargo build --workspace` also works on the desktop.

// Outside wasm32 it is only needed by tests.
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
    eprintln!("project-web must be built for wasm32: use `trunk serve` (see README).");
}
