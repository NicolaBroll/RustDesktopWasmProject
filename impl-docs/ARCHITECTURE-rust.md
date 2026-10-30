# Layered architecture in Rust: desktop and WebAssembly

> In this project the WebAssembly target has been removed (S31d, 2026-10-24): BinariGit is desktop
> only. The web parts of this guide (`myapp-web`, trunk, the wasm check in CI) do not apply.

Starting guide for a new project structured like PdfCraft and GridCraft: an engine independent of the UI, numbered layers with dependencies only downward, a single egui frontend compiled native and wasm.

Conventions: `myapp` is the placeholder name. Dependency versions and some signatures (in particular `eframe` and `cargo_metadata`) change often: **verify every snippet against the version you install**. The snippets are starting outlines, not tested code.

## 1. Layers

| Layer | Crate | Contents |
| --- | --- | --- |
| L0 | `myapp-core` | base types, errors, input parsing |
| L2 | `myapp-domain` | data model, file formats, domain logic |
| L5 | `myapp-engine` | session, command catalog, undo, I/O via traits |
| L6 | `myapp-ui` | egui interface, target-independent |
| L6 | `myapp-mcp` | MCP server |
| L7 | `myapp`, `myapp-cli`, `myapp-web` | thin executables: startup and wiring |

Numbers L1, L3 and L4 stay free for future layers (for example a toolkit-neutral renderer). The only rule: a crate imports only from **strictly lower** layers. Two crates of the same layer do not import each other.

## 2. Repository structure

```
myapp/
├─ Cargo.toml                 workspace
├─ .cargo/config.toml         alias for xtask
├─ crates/
│  ├─ core/        (myapp-core)
│  ├─ domain/      (myapp-domain)
│  ├─ engine/      (myapp-engine)
│  ├─ ui/          (myapp-ui)
│  └─ mcp/         (myapp-mcp)
├─ apps/
│  ├─ myapp/       desktop
│  ├─ myapp-cli/
│  └─ myapp-web/   wasm, built with trunk
├─ xtask/          ci, layers, release
├─ packaging/  docs/  assets/  .github/
└─ AGENTS.md  ATTRIBUTION.md  ROADMAP.md  clippy.toml  rustfmt.toml  deny.toml
```

Root `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/*", "apps/*", "xtask"]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
myapp-core   = { path = "crates/core" }
myapp-domain = { path = "crates/domain" }
myapp-engine = { path = "crates/engine" }
myapp-ui     = { path = "crates/ui" }
myapp-mcp    = { path = "crates/mcp" }
```

`.cargo/config.toml`:

```toml
[alias]
xtask = "run --package xtask --"
```

## 3. Automatic layering check (`xtask layers`)

Dependencies of `xtask/Cargo.toml`: `cargo_metadata`, `anyhow`.

```rust
use cargo_metadata::{DependencyKind, MetadataCommand};
use std::collections::HashMap;

fn levels() -> HashMap<&'static str, u8> {
    HashMap::from([
        ("myapp-core", 0),
        ("myapp-domain", 2),
        ("myapp-engine", 5),
        ("myapp-ui", 6),
        ("myapp-mcp", 6),
        ("myapp", 7),
        ("myapp-cli", 7),
        ("myapp-web", 7),
    ])
}

pub fn check_layers() -> anyhow::Result<()> {
    let metadata = MetadataCommand::new().exec()?;
    let levels = levels();
    let mut errors = Vec::new();

    for pkg in metadata.workspace_packages() {
        let name = pkg.name.to_string();
        let Some(&mine) = levels.get(name.as_str()) else {
            if name != "xtask" {
                errors.push(format!("{name}: layer not declared"));
            }
            continue;
        };
        for dep in pkg.dependencies.iter().filter(|d| d.kind == DependencyKind::Normal) {
            if let Some(&theirs) = levels.get(dep.name.as_str()) {
                if theirs >= mine {
                    errors.push(format!("{name} (L{mine}) depends on {} (L{theirs})", dep.name));
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("layering violations:\n  {}", errors.join("\n  "))
    }
}
```

`cargo xtask ci` runs in order: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `check_layers()` and `cargo check -p myapp-ui --target wasm32-unknown-unknown`. The same command should be used locally and in the CI workflow.

## 4. Engine: state, commands, I/O

### Copy-on-write state and undo

```rust
// crates/engine/src/session.rs
use std::sync::Arc;
use myapp_domain::Document;

pub struct Session {
    current: Arc<Document>,
    undo: Vec<Arc<Document>>,
    redo: Vec<Arc<Document>>,
}

impl Session {
    pub fn new(doc: Document) -> Self {
        Self { current: Arc::new(doc), undo: vec![], redo: vec![] }
    }

    /// Every modification goes through here: nearly free snapshot thanks to Arc.
    pub fn edit<R>(&mut self, f: impl FnOnce(&mut Document) -> R) -> R {
        self.undo.push(Arc::clone(&self.current));
        self.redo.clear();
        f(Arc::make_mut(&mut self.current))
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(&mut self.current, prev));
                true
            }
            None => false,
        }
    }

    pub fn document(&self) -> &Document {
        &self.current
    }
}
```

`Document` must implement `Clone`. For large documents replace the fields with persistent structures (crate `im` or `rpds`) to make cloning really cheap.

### Command catalog

A command has a stable id and JSON parameters. UI, CLI, MCP and tests use the same registry.
The registry deserializes the parameters into the handler's type, so the handler never parses
JSON and an invalid parameter is an `InvalidParams` error with the command's id.

```rust
// crates/engine/src/commands.rs
use crate::{EngineError, Session};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;

pub struct CommandSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub params_help: &'static str,
    /// JSON Schema of `P`, for CLI and MCP.
    pub params: Value,
    pub writes: bool,
}

pub enum Access { Read, Write }

type Handler = Box<dyn Fn(&mut Session, Value) -> Result<Value, EngineError> + Send + Sync>;

#[derive(Default)]
pub struct Registry {
    map: BTreeMap<&'static str, (CommandSpec, Handler)>,
}

impl Registry {
    pub fn add<P: DeserializeOwned + JsonSchema + 'static>(
        &mut self,
        id: &'static str,
        title: &'static str,
        params_help: &'static str,
        access: Access,
        handler: fn(&mut Session, P) -> Result<Value, EngineError>,
    ) {
        let params = schemars::schema_for!(P).into();
        let spec = CommandSpec { id, title, params_help, params, writes: matches!(access, Access::Write) };
        let run: Handler = Box::new(move |session, params| {
            let params = serde_json::from_value(params)
                .map_err(|e| EngineError::InvalidParams { id, reason: e.to_string() })?;
            handler(session, params)
        });
        self.map.insert(id, (spec, run));
    }

    pub fn list(&self) -> impl Iterator<Item = &CommandSpec> {
        self.map.values().map(|(spec, _)| spec)
    }

    pub fn run(&self, id: &str, session: &mut Session, params: Value) -> Result<Value, EngineError> {
        let (_, handler) = self.map.get(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?;
        handler(session, params)
    }
}
```

Rule: every button, menu item and shortcut in the UI invokes `Registry::run`. No modification logic inside graphical callbacks.

### Platform interface

```rust
// crates/engine/src/platform.rs
#[derive(Clone, Copy, Default)]
pub struct Capabilities { pub open_folder: bool }

pub trait Platform: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    /// Does not block: the result arrives via callback, possibly from another thread.
    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>);
    /// Runs a slow job off the UI thread.
    fn spawn(&self, job: Box<dyn FnOnce() + Send>);
}
```

Native implementation in the desktop app: `rfd::AsyncFileDialog` parented to the window, awaited on a
job thread, so the window keeps repainting while the dialog is open. The UI reads `capabilities()` and hides unavailable commands.

## 5. Interface and two targets

`crates/ui` exposes `App`, which implements `eframe::App` and receives an `Arc<dyn Platform>`; it contains no `cfg(target_arch)` except in exceptional cases.

### Desktop (`apps/myapp/src/main.rs`)

```rust
use std::sync::Arc;

fn main() -> eframe::Result {
    let platform = Arc::new(NativePlatform::default());
    eframe::run_native(
        "MyApp",
        eframe::NativeOptions::default(),
        // The creator's signature changes between eframe versions: Ok(Box::new(..)) in recent ones.
        Box::new(move |cc| Ok(Box::new(myapp_ui::App::new(cc, platform.clone())))),
    )
}
```

### Web (`apps/myapp-web/src/main.rs`)

```rust
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;
    use std::sync::Arc;

    wasm_bindgen_futures::spawn_local(async {
        let canvas = web_sys::window().unwrap().document().unwrap()
            .get_element_by_id("app_canvas").unwrap()
            .dyn_into::<web_sys::HtmlCanvasElement>().unwrap();

        eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(myapp_ui::App::new(cc, Arc::new(WebPlatform::default()))))),
            )
            .await
            .expect("startup failed");
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
```

`apps/myapp-web/index.html`:

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>MyApp</title>
  <link data-trunk rel="rust" />
</head>
<body>
  <canvas id="app_canvas"></canvas>
</body>
</html>
```

Web build: install `trunk` and the target (`rustup target add wasm32-unknown-unknown`), then `cd apps/myapp-web && trunk serve --release`. For release, `trunk build --release` produces a static site.

Note: the egui UI on the web is drawn on a canvas. Accessibility, text selection, IME and paste deserve an early prototype.

## 6. Surface for agents

- **CLI** (`myapp-cli`): `commands --search <text>`, `run --cmd <id>=<json> --out file`, `eval`. Uses `clap`.
- **Server MCP** (`myapp-mcp`): starts only with `myapp-cli mcp`, talks over stdin/stdout, no network port, `--root <dir>` option that confines all paths. It exposes tools such as `list_commands` and `execute_command`, which reaches the whole catalog.
- **UI control** (optional, later): off by default, loopback only, token in a file readable only by the user.

## 7. Quality, licenses, packaging

- `rustfmt`, Clippy with warnings as errors, unit and property tests (`proptest`) on the engine.
- `cargo deny` with `deny.toml` for licenses and vulnerabilities; `ATTRIBUTION.md` for every asset.
- Corpus of real files and, where one exists, an independent implementation as a comparison oracle.
- Per-platform packages (MSI, DMG, AppImage, Flatpak, deb, rpm, zip of the web site) with `SHA256SUMS.txt`; signing on Windows and macOS. Start from two desktop platforms and the web.

## 8. First iteration (checklist)

- [ ] Workspace with `core`, `engine`, `ui` and the three apps, even with just an empty window.
- [ ] `cargo xtask ci` green: formatting, lint, tests, layering, `cargo check` for wasm.
- [ ] The same window opens on desktop and with `trunk serve`.
- [ ] `Platform` defined with `pick_file` and `save_file` and the two implementations.
- [ ] One sample command in the registry, run from both the UI and the CLI.

## 9. To verify before starting

- Current versions of `eframe`, `egui`, `wasm-bindgen`, `trunk` and `cargo_metadata`.
- Which graphics backend `eframe` uses for the web in the chosen version (wgpu or glow).
- Whether the chosen domain already has a mature crate to use instead of writing everything from scratch; if so, isolate it behind an interface and declare it in the README.
