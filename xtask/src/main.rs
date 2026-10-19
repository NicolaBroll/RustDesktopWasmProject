//! `cargo xtask <compito>`: lo stesso comando in locale e in CI.

mod layers;

use anyhow::{Context as _, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

const WASM_TARGET: &str = "wasm32-unknown-unknown";

const USAGE: &str = "\
uso: cargo xtask <compito>

  ci        fmt, clippy, test, layering, clippy wasm
  layers    verifica che ogni crate dipenda solo da livelli inferiori
  release   build desktop/CLI ottimizzata e sito web in dist/";

fn main() -> anyhow::Result<()> {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("ci") => ci(),
        Some("layers") => layers::check_layers(),
        Some("release") => release(),
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("compito sconosciuto: {other}\n\n{USAGE}"),
    }
}

fn ci() -> anyhow::Result<()> {
    cargo(&["fmt", "--all", "--check"])?;
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])?;
    cargo(&["test", "--workspace"])?;
    step("layering");
    layers::check_layers()?;
    // L'interfaccia e l'app web devono compilare anche per il browser.
    cargo(&[
        "clippy",
        "-p",
        "project-ui",
        "-p",
        "project-web",
        "--target",
        WASM_TARGET,
        "--",
        "-D",
        "warnings",
    ])?;
    eprintln!("\nci: tutto verde");
    Ok(())
}

fn release() -> anyhow::Result<()> {
    cargo(&["build", "--release", "-p", "project", "-p", "project-cli"])?;
    run(Command::new("trunk").args(["build", "--release"]))
}

fn cargo(args: &[&str]) -> anyhow::Result<()> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    run(Command::new(cargo).args(args))
}

fn run(cmd: &mut Command) -> anyhow::Result<()> {
    cmd.current_dir(workspace_root());
    step(&format!("{cmd:?}"));
    let status = cmd
        .status()
        .with_context(|| format!("avvio di {:?}", cmd.get_program()))?;
    if !status.success() {
        bail!("{cmd:?} è fallito ({status})");
    }
    Ok(())
}

fn step(name: &str) {
    eprintln!("\n==> {name}");
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sta dentro il workspace")
        .to_path_buf()
}
