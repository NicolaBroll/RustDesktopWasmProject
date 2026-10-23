//! `cargo xtask <compito>`: lo stesso comando in locale e in CI.

mod layers;

use anyhow::{Context as _, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// `--quiet`: l'output dei comandi si mostra solo se falliscono (meno testo da leggere per chi
/// lancia la CI tante volte, come gli agenti).
static QUIET: AtomicBool = AtomicBool::new(false);

const USAGE: &str = "\
uso: cargo xtask <compito>

  ci        fmt, clippy, test, layering, clippy wasm
            (`ci --quiet`: una riga per passo, l'output solo del passo che fallisce)
  layers    verifica che ogni crate dipenda solo da livelli inferiori
  release   build desktop/CLI ottimizzata e sito web in dist/";

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let task = args.next();
    match task.as_deref() {
        Some("ci") => {
            for arg in args {
                match arg.as_str() {
                    "-q" | "--quiet" => QUIET.store(true, Ordering::Relaxed),
                    other => bail!("opzione sconosciuta per ci: {other}\n\n{USAGE}"),
                }
            }
            ci()
        }
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
    // Con `--quiet` anche i test stampano solo i fallimenti, se l'output va mostrato.
    if QUIET.load(Ordering::Relaxed) {
        cargo(&["test", "--workspace", "-q"])?;
    } else {
        cargo(&["test", "--workspace"])?;
    }
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
    if QUIET.load(Ordering::Relaxed) {
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
        step(&args.join(" "));
        let output = cmd
            .output()
            .with_context(|| format!("avvio di {:?}", cmd.get_program()))?;
        if !output.status.success() {
            use std::io::Write as _;
            std::io::stderr().write_all(&output.stdout)?;
            std::io::stderr().write_all(&output.stderr)?;
            bail!("{cmd:?} è fallito ({})", output.status);
        }
        return Ok(());
    }
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
    let gap = if QUIET.load(Ordering::Relaxed) {
        ""
    } else {
        "\n"
    };
    eprintln!("{gap}==> {name}");
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sta dentro il workspace")
        .to_path_buf()
}
