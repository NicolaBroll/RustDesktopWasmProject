//! `cargo xtask <task>`: the same command locally and in CI.

mod layers;

use anyhow::{Context as _, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

/// `--quiet`: command output is shown only if they fail (less text to read for whoever
/// runs CI many times, such as agents).
static QUIET: AtomicBool = AtomicBool::new(false);

const USAGE: &str = "\
usage: cargo xtask <task>

  ci        fmt, clippy, test, doc, layering, cargo deny (licenses, bans, sources)
            (`ci --quiet`: one line per step, output only of the failing step)
  layers    checks that every crate depends only on lower layers
  release   optimized desktop/CLI build";

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let task = args.next();
    match task.as_deref() {
        Some("ci") => {
            for arg in args {
                match arg.as_str() {
                    "-q" | "--quiet" => QUIET.store(true, Ordering::Relaxed),
                    other => bail!("unknown option for ci: {other}\n\n{USAGE}"),
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
        Some(other) => bail!("unknown task: {other}\n\n{USAGE}"),
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
    // With `--quiet` tests also print only failures, if the output is to be shown.
    if QUIET.load(Ordering::Relaxed) {
        cargo(&["test", "--workspace", "-q"])?;
    } else {
        cargo(&["test", "--workspace"])?;
    }
    doc()?;
    step("layering");
    layers::check_layers()?;
    deny()?;
    eprintln!("\nci: all green");
    Ok(())
}

/// `cargo deny check licenses bans sources` (`deny.toml`): no network, always the same result.
/// Advisories change every day and run in `.github/workflows/audit.yml` instead. Without
/// `cargo-deny` installed the step is skipped with a warning, except in CI (`CI` set), where it
/// fails.
fn deny() -> anyhow::Result<()> {
    let program = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let installed = Command::new(program)
        .args(["deny", "--version"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !installed {
        if std::env::var_os("CI").is_some() {
            bail!("cargo-deny is not installed (`make setup`)");
        }
        step("cargo deny: skipped, not installed (`make setup` installs it)");
        return Ok(());
    }
    cargo(&[
        "deny",
        "--log-level",
        "error",
        "check",
        "licenses",
        "bans",
        "sources",
    ])
}

/// `cargo doc` with warnings as errors, private items included: broken intra-doc links
/// (renamed or removed items) fail CI instead of rotting.
fn doc() -> anyhow::Result<()> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    run(Command::new(cargo)
        .args(["doc", "--workspace", "--no-deps", "--document-private-items"])
        .env("RUSTDOCFLAGS", "-D warnings"))
}

fn release() -> anyhow::Result<()> {
    cargo(&["build", "--release", "-p", "project", "-p", "project-cli"])
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
            .with_context(|| format!("starting {:?}", cmd.get_program()))?;
        if !output.status.success() {
            use std::io::Write as _;
            std::io::stderr().write_all(&output.stdout)?;
            std::io::stderr().write_all(&output.stderr)?;
            bail!("{cmd:?} failed ({})", output.status);
        }
        return Ok(());
    }
    step(&format!("{cmd:?}"));
    let status = cmd
        .status()
        .with_context(|| format!("starting {:?}", cmd.get_program()))?;
    if !status.success() {
        bail!("{cmd:?} failed ({status})");
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
        .expect("xtask is inside the workspace")
        .to_path_buf()
}
